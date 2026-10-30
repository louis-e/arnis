//! Bedrock Edition .mcworld format world saving.
//!
//! This module handles saving worlds in the Bedrock Edition format,
//! producing .mcworld files that can be imported into Minecraft Bedrock.

use super::bedrock_entities;
use super::common::{BlockStorage, ChunkToModify, SectionToModify, WorldToModify};
use super::WorldMetadata;
use crate::bedrock_block_map::{
    is_waterlogged, potted_plant, to_bedrock_block_with_properties, BedrockBlock,
    BedrockBlockStateValue, BLOCK_STATE_VERSION,
};
use crate::block_definitions::{Block, AIR};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::progress::emit_gui_progress_update;
use crate::world_utils::{check_disk_space, estimated_world_bytes, WorldSizeOptions};

use bedrockrs_level::level::db_interface::key_level::KeyTypeTag;
use bedrockrs_level::level::db_interface::rusty::mcpe_options;
use bedrockrs_shared::world::dimension::Dimension;
use byteorder::{LittleEndian, WriteBytesExt};
use fastnbt::Value;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use rusty_leveldb::env::{Env, FileLock, Logger, RandomAccess};
use rusty_leveldb::{Options, PosixDiskEnv, DB};
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap as StdHashMap;
use std::fs::{self, File};
use std::io::{Cursor, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
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
    Database(rusty_leveldb::Status),
    Nbt(String),
}

impl BedrockSaveError {
    /// The cause in a few words, for the GUI's one-line status.
    pub fn reason(&self) -> &'static str {
        use std::io::ErrorKind;
        let kind = match self {
            BedrockSaveError::Io(e) | BedrockSaveError::Zip(zip::result::ZipError::Io(e)) => {
                Some(e.kind())
            }
            BedrockSaveError::Database(s)
                if s.code == rusty_leveldb::StatusCode::PermissionDenied =>
            {
                Some(ErrorKind::PermissionDenied)
            }
            // rusty-leveldb keeps only the message of other io errors.
            BedrockSaveError::Database(s)
                if s.err.contains("os error 28") || s.err.contains("os error 112") =>
            {
                Some(ErrorKind::StorageFull)
            }
            _ => None,
        };
        match (kind, self) {
            (Some(ErrorKind::PermissionDenied), _) => "access denied",
            (Some(ErrorKind::StorageFull), _) => "disk is full",
            (Some(ErrorKind::AlreadyExists), _) => "folder in the way",
            (_, BedrockSaveError::Database(_)) => "LevelDB error",
            (_, BedrockSaveError::Io(_) | BedrockSaveError::Zip(_)) => "file error",
            (_, BedrockSaveError::Serialization(_) | BedrockSaveError::Nbt(_)) => "encoding error",
        }
    }
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
            BedrockSaveError::Database(status) => {
                write!(f, "LevelDB error: {status}")
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

impl From<rusty_leveldb::Status> for BedrockSaveError {
    fn from(status: rusty_leveldb::Status) -> Self {
        BedrockSaveError::Database(status)
    }
}

const DEFAULT_BEDROCK_COMPRESSION_LEVEL: u8 = 6;

/// The chunk format of 1.21.120 and later.
const CHUNK_VERSION: u8 = 42;

/// The version the world claims and needs: 1.21.120, the first with chunk format 42.
const SAVED_WITH_VERSION: [i32; 5] = [1, 21, 120, 0, 0];
const SAVED_WITH_INVENTORY_VERSION: &str = "1.21.120";
const SAVED_WITH_PROTOCOL: i32 = 859;

/// rusty-leveldb 3.0.3's LRU `remove` leaves the list's tail pointer dangling
/// when it removes the last node, and the table cache is the only cache that
/// removes (on every compaction). A table cache of one entry never has a tail
/// apart from its head, so the dangling pointer is reset by the next insert
/// before anything reads it. The library reserves 10 handles, so 11 gives 1.
/// Fixed upstream after 4.0.1; drop this once bedrockrs moves to a fixed release.
const LEVELDB_MAX_OPEN_FILES: usize = 11;

/// LevelDB options for writing a Bedrock world. `mcpe_options` sets the
/// compressors; this adds the workarounds for rusty-leveldb panics.
fn bedrock_db_options() -> Options {
    let mut opts = mcpe_options(DEFAULT_BEDROCK_COMPRESSION_LEVEL);
    opts.env = Rc::new(Box::new(MonotonicDiskEnv::new()));
    opts.max_open_files = LEVELDB_MAX_OPEN_FILES;
    opts
}

/// Tries at a file that a virus scanner or sync client holds for a moment, about 1.5 s in all.
const TRANSIENT_ATTEMPTS: u32 = 10;

/// Whether an error is such a hold. Only Windows refuses a file another process has open.
fn is_transient_io_error(e: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION.
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Runs `op` again while `transient` calls its error a passing hold.
fn retry_while<T, E>(
    mut op: impl FnMut() -> Result<T, E>,
    transient: impl Fn(&E) -> bool,
) -> Result<T, E> {
    let mut delay = std::time::Duration::from_millis(10);
    let mut attempt = 1;
    loop {
        match op() {
            Err(e) if attempt < TRANSIENT_ATTEMPTS && transient(&e) => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(std::time::Duration::from_millis(250));
                attempt += 1;
            }
            result => return result,
        }
    }
}

fn retry_transient<T>(op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    retry_while(op, is_transient_io_error)
}

/// An io error as rusty-leveldb reports it, naming the operation and the file.
fn leveldb_status(method: &str, path: &Path, e: std::io::Error) -> rusty_leveldb::Status {
    let mut status = rusty_leveldb::Status::from(e);
    status.err = format!("{method}: {}: {}", status.err, path.display());
    status
}

/// The stock disk env with retries on file opens, renames and deletes, and a monotonic
/// clock, since rusty-leveldb's compaction stats overflow when the wall clock steps back.
struct MonotonicDiskEnv {
    inner: PosixDiskEnv,
    epoch: Instant,
}

impl MonotonicDiskEnv {
    fn new() -> Self {
        Self {
            inner: PosixDiskEnv::new(),
            epoch: Instant::now(),
        }
    }
}

impl Env for MonotonicDiskEnv {
    fn open_sequential_file(&self, p: &Path) -> rusty_leveldb::Result<Box<dyn std::io::Read>> {
        retry_transient(|| File::open(p))
            .map(|f| Box::new(f) as Box<dyn std::io::Read>)
            .map_err(|e| leveldb_status("open (seq)", p, e))
    }
    fn open_random_access_file(&self, p: &Path) -> rusty_leveldb::Result<Box<dyn RandomAccess>> {
        retry_transient(|| File::open(p))
            .map(|f| Box::new(f) as Box<dyn RandomAccess>)
            .map_err(|e| leveldb_status("open (randomaccess)", p, e))
    }
    fn open_writable_file(&self, p: &Path) -> rusty_leveldb::Result<Box<dyn IoWrite>> {
        retry_transient(|| {
            fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(p)
        })
        .map(|f| Box::new(f) as Box<dyn IoWrite>)
        .map_err(|e| leveldb_status("open (write)", p, e))
    }
    fn open_appendable_file(&self, p: &Path) -> rusty_leveldb::Result<Box<dyn IoWrite>> {
        retry_transient(|| fs::OpenOptions::new().create(true).append(true).open(p))
            .map(|f| Box::new(f) as Box<dyn IoWrite>)
            .map_err(|e| leveldb_status("open (append)", p, e))
    }
    fn exists(&self, p: &Path) -> rusty_leveldb::Result<bool> {
        self.inner.exists(p)
    }
    fn children(&self, p: &Path) -> rusty_leveldb::Result<Vec<PathBuf>> {
        self.inner.children(p)
    }
    fn size_of(&self, p: &Path) -> rusty_leveldb::Result<usize> {
        self.inner.size_of(p)
    }
    fn delete(&self, p: &Path) -> rusty_leveldb::Result<()> {
        retry_transient(|| fs::remove_file(p)).map_err(|e| leveldb_status("delete", p, e))
    }
    fn mkdir(&self, p: &Path) -> rusty_leveldb::Result<()> {
        self.inner.mkdir(p)
    }
    fn rmdir(&self, p: &Path) -> rusty_leveldb::Result<()> {
        self.inner.rmdir(p)
    }
    fn rename(&self, from: &Path, to: &Path) -> rusty_leveldb::Result<()> {
        retry_transient(|| fs::rename(from, to)).map_err(|e| leveldb_status("rename", from, e))
    }
    fn lock(&self, p: &Path) -> rusty_leveldb::Result<FileLock> {
        // Creating the lock file can meet the same holds; a held lock is LockError.
        retry_while(
            || self.inner.lock(p),
            |e| {
                cfg!(windows)
                    && matches!(
                        e.code,
                        rusty_leveldb::StatusCode::PermissionDenied
                            | rusty_leveldb::StatusCode::IOError
                    )
            },
        )
    }
    fn unlock(&self, l: FileLock) -> rusty_leveldb::Result<()> {
        self.inner.unlock(l)
    }
    fn new_logger(&self, p: &Path) -> rusty_leveldb::Result<Logger> {
        self.inner.new_logger(p)
    }
    fn micros(&self) -> u64 {
        // Offset by one so the first reading is never 0, like a wall clock's.
        self.epoch.elapsed().as_micros() as u64 + 1
    }
    fn sleep_for(&self, micros: u32) {
        self.inner.sleep_for(micros)
    }
}

/// Marks the staging directory as ours. Never packaged, `package_mcworld` names its files.
const STAGING_MARKER: &str = ".arnis-staging";

/// A private staging folder untouched this long belongs to a run that died.
const STALE_STAGING_SECS: u64 = 24 * 60 * 60;

/// The folder a world is assembled in before it is packaged. Removed when dropped.
struct Staging {
    dir: PathBuf,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "Warning: could not remove the Bedrock staging folder '{}': {e}",
                    self.dir.display()
                );
            }
        }
    }
}

/// The local cache, which no sync client watches and which is never a RAM disk.
fn private_staging_root() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join("arnis-test-bedrock-staging"));
    }
    dirs::cache_dir().map(|dir| dir.join("arnis").join("bedrock-staging"))
}

/// Removes the staging folders of runs that died, i.e. untouched for a day.
fn remove_stale_staging(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let max_age = std::time::Duration::from_secs(STALE_STAGING_SECS);
    for entry in entries.flatten() {
        let path = entry.path();
        let last_write = [path.clone(), path.join("db")]
            .iter()
            .filter_map(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
            .max();
        let stale = last_write
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > max_age);
        if stale && path.join(STAGING_MARKER).is_file() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

/// A LevelDB (key, value) record, and all records for one chunk.
type DbRecord = (Vec<u8>, Vec<u8>);
type ChunkRecords = Vec<DbRecord>;

/// The potted plants of a section by section index.
type PottedPlants = Vec<(usize, BedrockBlock)>;

/// A section in Bedrock's terms: its blocks, the water of waterlogged cells, its pots.
struct BedrockSection {
    palette: Vec<BedrockBlock>,
    indices: Box<[u16; 4096]>,
    water: Option<Box<[bool; 4096]>>,
    pots: PottedPlants,
}

/// Prepares one of the folders a world can be staged in.
type PrepareStaging = fn(&BedrockWriter) -> Result<Staging, BedrockSaveError>;

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
    states: BTreeMap<String, Value>,
    version: i32,
}

/// Writer for Bedrock Edition worlds
pub struct BedrockWriter {
    mcworld_path: PathBuf,
    /// World position of the ground grid's first cell; the area's corner when unset.
    ground_origin: Option<(i32, i32)>,
    /// Latitude of the area's centre, which picks temperate biome variants.
    center_lat: f64,
    level_name: String,
    spawn_point: Option<(i32, i32)>,
    ground: Option<Arc<Ground>>,
    extend_build_height: bool,
    projection: String,
    scale: f64,
    game_mode: crate::args::GameMode,
    world_time: i64,
    start_with_map: bool,
    mob_spawning: crate::args::MobSpawning,
    size_options: WorldSizeOptions,
    addon: Option<crate::bedrock_addon::Addon>,
}

impl BedrockWriter {
    /// Creates a new BedrockWriter
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        output_path: PathBuf,
        level_name: String,
        spawn_point: Option<(i32, i32)>,
        ground: Option<Arc<Ground>>,
        extend_build_height: bool,
        projection: String,
        scale: f64,
        game_mode: crate::args::GameMode,
        world_time: i64,
        start_with_map: bool,
    ) -> Self {
        let mcworld_path = if output_path.extension().is_some_and(|ext| ext == "mcworld") {
            output_path
        } else {
            append_mcworld_extension(&output_path)
        };

        Self {
            mcworld_path,
            ground_origin: None,
            center_lat: 0.0,
            level_name,
            spawn_point,
            ground,
            extend_build_height,
            projection,
            scale,
            game_mode,
            world_time,
            start_with_map,
            mob_spawning: crate::args::MobSpawning::Off,
            size_options: WorldSizeOptions::default(),
            addon: None,
        }
    }

    /// Packs to ship inside the world, as the moving vehicles.
    pub fn with_addon(mut self, addon: Option<crate::bedrock_addon::Addon>) -> Self {
        self.addon = addon;
        self
    }

    /// Where the ground grid starts, which the biomes and the spawn height read it from.
    pub fn with_ground_origin(mut self, origin: (i32, i32)) -> Self {
        self.ground_origin = Some(origin);
        self
    }

    /// What spawns on its own, which also picks the difficulty.
    pub fn with_mob_spawning(mut self, mobs: crate::args::MobSpawning) -> Self {
        self.mob_spawning = mobs;
        self
    }

    /// The run's options, which size the world for the staging check.
    pub fn with_size_options(mut self, options: WorldSizeOptions) -> Self {
        self.size_options = options;
        self
    }

    /// Writes the world to disk
    pub fn write_world(
        &mut self,
        world: &WorldToModify,
        xzbbox: &XZBBox,
        llbbox: &LLBBox,
    ) -> Result<(), BedrockSaveError> {
        self.ground_origin
            .get_or_insert((xzbbox.min_x(), xzbbox.min_z()));
        self.center_lat = (llbbox.min().lat() + llbbox.max().lat()) * 0.5;
        self.remove_legacy_staging();
        let staging = self.stage_world(world, xzbbox, llbbox)?;

        emit_gui_progress_update(98.0, "Saving Bedrock world...");
        self.package_mcworld(&staging.dir)?;

        emit_gui_progress_update(99.0, "Saving Bedrock world...");
        drop(staging);
        Ok(())
    }

    /// Assembles the world in the private staging folder, or beside the archive if that fails
    /// or the cache drive is short of room.
    fn stage_world(
        &self,
        world: &WorldToModify,
        xzbbox: &XZBBox,
        llbbox: &LLBBox,
    ) -> Result<Staging, BedrockSaveError> {
        let candidates: [(PrepareStaging, bool); 2] = [
            (Self::private_staging, self.private_staging_fits(xzbbox)),
            (Self::staging_beside_archive, true),
        ];
        // The first failure is reported: the fallback's own trouble rarely explains it.
        let mut failure = None;
        for (prepare, _) in candidates.into_iter().filter(|&(_, usable)| usable) {
            let staging = match prepare(self) {
                Ok(staging) => staging,
                Err(e) => {
                    eprintln!("Warning: could not create a Bedrock staging folder: {e}");
                    failure.get_or_insert(e);
                    continue;
                }
            };
            match self.write_staging(&staging.dir, world, xzbbox, llbbox) {
                Ok(()) => return Ok(staging),
                // Only the disk can fail differently somewhere else.
                Err(e @ (BedrockSaveError::Io(_) | BedrockSaveError::Database(_))) => {
                    eprintln!(
                        "Warning: staging the Bedrock world in '{}' failed: {e}",
                        staging.dir.display()
                    );
                    failure.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(failure.expect("at least one staging candidate"))
    }

    fn write_staging(
        &self,
        dir: &Path,
        world: &WorldToModify,
        xzbbox: &XZBBox,
        llbbox: &LLBBox,
    ) -> Result<(), BedrockSaveError> {
        fs::write(dir.join("levelname.txt"), &self.level_name)?;

        emit_gui_progress_update(91.0, "Saving Bedrock world...");
        self.write_level_dat(dir, xzbbox)?;

        emit_gui_progress_update(92.0, "Saving Bedrock world...");
        self.write_chunks_to_db(dir, world)?;

        emit_gui_progress_update(97.0, "Saving Bedrock world...");
        self.write_metadata(dir, world, xzbbox, llbbox)
    }

    /// Whether the cache drive, which the run's own disk check never measured, takes this world.
    fn private_staging_fits(&self, xzbbox: &XZBBox) -> bool {
        let Some(root) = private_staging_root() else {
            return true;
        };
        let km2 = xzbbox.bounding_rect().total_blocks() as f64 / 1e6;
        let options = WorldSizeOptions {
            tall: self.extend_build_height,
            ..self.size_options
        };
        let bound = estimated_world_bytes(km2, super::WorldFormat::BedrockMcWorld, options);
        check_disk_space(&root, bound).is_ok()
    }

    /// A fresh folder in the local cache, away from a Desktop that OneDrive syncs.
    fn private_staging(&self) -> Result<Staging, BedrockSaveError> {
        let root = private_staging_root().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no cache directory")
        })?;
        fs::create_dir_all(&root)?;
        remove_stale_staging(&root);
        let dir = root.join(format!("world-{}", crate::world_utils::unique_suffix()));
        fs::create_dir(&dir)?;
        let staging = Staging { dir };
        fs::write(staging.dir.join(STAGING_MARKER), b"")?;
        Ok(staging)
    }

    /// `<name>.mcworld.staging`, never the bare name, which unpacking the archive creates.
    fn staging_beside_archive(&self) -> Result<Staging, BedrockSaveError> {
        let dir = self.beside_archive_staging_dir();
        if dir.exists() {
            if !is_arnis_staging_dir(&dir) {
                return Err(BedrockSaveError::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "'{}' already exists and was not created by Arnis. \
                         Rename or move it, or choose a different Bedrock save path.",
                        dir.display()
                    ),
                )));
            }
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        let staging = Staging { dir };
        fs::write(staging.dir.join(STAGING_MARKER), b"")?;
        Ok(staging)
    }

    fn beside_archive_staging_dir(&self) -> PathBuf {
        let mut staging = self.mcworld_path.clone().into_os_string();
        staging.push(".staging");
        PathBuf::from(staging)
    }

    /// Removes marked staging folders earlier runs left beside the archive.
    fn remove_legacy_staging(&self) {
        for dir in [
            self.mcworld_path.with_extension(""),
            self.beside_archive_staging_dir(),
        ] {
            if dir.join(STAGING_MARKER).is_file() {
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }

    fn write_level_dat(&self, dir: &Path, xzbbox: &XZBBox) -> Result<(), BedrockSaveError> {
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
                // Ground elevation data is indexed from the ground grid's origin
                let (origin_x, origin_z) = self
                    .ground_origin
                    .unwrap_or((xzbbox.min_x(), xzbbox.min_z()));
                let rel_x = spawn_x - origin_x;
                let rel_z = spawn_z - origin_z;
                let coord = crate::coordinate_system::cartesian::XZPoint::new(rel_x, rel_z);
                ground.level(coord) + 3 // Add 3 blocks above ground for safety
            })
            .unwrap_or(64);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mobs = self.mob_spawning;

        // Build complete level.dat NBT structure
        let level_dat = BedrockLevelDat {
            // Version information - critical for Bedrock to recognize the world
            storage_version: 10,
            network_version: SAVED_WITH_PROTOCOL,
            world_version: 1,
            inventory_version: SAVED_WITH_INVENTORY_VERSION.to_string(),
            last_opened_with_version: SAVED_WITH_VERSION.to_vec(),
            minimum_compatible_client_version: SAVED_WITH_VERSION.to_vec(),

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
            spawn_mobs: mobs.spawns(),

            // Game settings
            game_type: self.game_mode.bedrock_game_type(),
            difficulty: mobs.bedrock_difficulty(),
            force_game_type: false,

            // Time
            last_played: now,
            time: self.world_time,
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
            do_mob_spawning: mobs.spawns(),
            do_mob_loot: true,
            do_tile_drops: true,
            do_entity_drops: true,
            do_fire_tick: true,
            // Off with monsters, so creepers and endermen leave the city standing.
            mob_griefing: !mobs.monsters(),
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
            start_with_map_enabled: self.start_with_map,
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
            // 26.30 replaced it with playerwaypoints and migrates it.
            locator_bar: true,
            tnt_explosion_drop_decay: true,
            // The tall pack needs no experiment since Custom Biomes left it in 1.21.110.
            experiments: BedrockExperiments {
                experiments_ever_used: false,
                saved_with_toggled_experiments: false,
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
        let mut file = File::create(dir.join("level.dat"))?;
        // Storage version: 10 (current Bedrock format)
        file.write_u32::<LittleEndian>(10)?;
        // Length of NBT data
        file.write_u32::<LittleEndian>(nbt_bytes.len() as u32)?;
        file.write_all(&nbt_bytes)?;

        Ok(())
    }

    fn write_chunks_to_db(
        &self,
        dir: &Path,
        world: &WorldToModify,
    ) -> Result<(), BedrockSaveError> {
        let db_path = dir.join("db");

        // Open LevelDB once for all writes (blocks, entities, block entities)
        let mut opts = bedrock_db_options();
        opts.create_if_missing = true;
        let mut db = DB::open(db_path.into_boxed_path(), opts)?;

        // Chunks per parallel-encode window. Bounds transient memory (only one window of encoded
        // bytes is held at a time) while giving rayon plenty of work per batch.
        const ENCODE_BATCH_CHUNKS: usize = 256;

        // Flatten to absolute-coord chunk refs (cheap — references only). Iteration order matches
        // the original nested loop, so LevelDB receives writes in the same order.
        let chunks: Vec<(i32, i32, &ChunkToModify)> = world
            .regions
            .iter()
            .flat_map(|((region_x, region_z), region)| {
                region
                    .chunks
                    .iter()
                    .map(move |((local_x, local_z), chunk)| {
                        (region_x * 32 + local_x, region_z * 32 + local_z, chunk)
                    })
            })
            .collect();

        let total_chunks = chunks.len();
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

        // Encode each window's chunk records in parallel (CPU-bound), then write them to LevelDB
        // sequentially — rusty_leveldb writes aren't concurrent-safe. Records are byte-identical
        // to the previous serial path; only the encoding is now multi-threaded.
        let mut chunks_processed: usize = 0;
        for window in chunks.chunks(ENCODE_BATCH_CHUNKS) {
            let encoded: Vec<ChunkRecords> = window
                .par_iter()
                .map(|&(abs_x, abs_z, chunk)| self.encode_chunk_records(abs_x, abs_z, chunk))
                .collect::<Result<_, _>>()?;

            for records in &encoded {
                for (key, value) in records {
                    db.put(key, value)?;
                }
                chunks_processed += 1;
                progress_bar.inc(1);
                // GUI progress spans 92%..97% for chunk writing.
                if chunks_processed.is_multiple_of(10) || chunks_processed == total_chunks {
                    let chunk_progress = chunks_processed as f64 / total_chunks as f64;
                    emit_gui_progress_update(92.0 + chunk_progress * 5.0, "");
                }
            }
        }

        // Closed here so a failed flush is an error, not missing chunks.
        db.close()?;
        progress_bar.finish_with_message("Chunks written to LevelDB");
        Ok(())
    }

    /// Encodes every LevelDB record for one chunk: version, Data3D, subchunks, then entities.
    fn encode_chunk_records(
        &self,
        abs_chunk_x: i32,
        abs_chunk_z: i32,
        chunk: &ChunkToModify,
    ) -> Result<ChunkRecords, BedrockSaveError> {
        let chunk_pos = Vec2::new(abs_chunk_x, abs_chunk_z);
        let mut records: ChunkRecords = Vec::with_capacity(chunk.sections.len() + 4);

        // Chunk format 42, current since 1.21.120.
        records.push((
            build_chunk_key_bytes(chunk_pos, Dimension::Overworld, KeyTypeTag::Version, None),
            vec![CHUNK_VERSION],
        ));
        // Fully generated, so the game neither populates nor regenerates the chunk.
        records.push((
            build_chunk_key_bytes(
                chunk_pos,
                Dimension::Overworld,
                KeyTypeTag::FinalizedState,
                None,
            ),
            2i32.to_le_bytes().to_vec(),
        ));
        // Data3D (heightmap + biomes), required for a valid chunk.
        records.push((
            build_chunk_key_bytes(chunk_pos, Dimension::Overworld, KeyTypeTag::Data3D, None),
            self.create_data3d(abs_chunk_x, abs_chunk_z, chunk),
        ));
        // Bedrock's item frames are blocks, so each needs an empty cell in a stored section;
        // one in a wall would cut a hole into it.
        let mut frame_cells = std::collections::HashSet::new();
        let frames: Vec<_> = compound_list(chunk.other.get("entities"))
            .filter_map(bedrock_entities::item_frame)
            .filter(|((x, y, z), _, _)| {
                x >> 4 == abs_chunk_x
                    && z >> 4 == abs_chunk_z
                    && chunk.sections.get((y >> 4) as i8).is_some()
                    && chunk
                        .get_block((x & 15) as u8, *y, (z & 15) as u8)
                        .is_none()
                    && frame_cells.insert((*x, *y, *z))
            })
            .collect();
        let mut overrides: StdHashMap<i8, StdHashMap<usize, BedrockBlock>> = StdHashMap::new();
        for ((x, y, z), block, _) in &frames {
            let index =
                (y.rem_euclid(16) * 256 + z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize;
            overrides
                .entry((y >> 4) as i8)
                .or_default()
                .insert(index, block.clone());
        }

        // Subchunks.
        let mut pots = Vec::new();
        for (section_y, section) in chunk.sections.iter() {
            let (bytes, section_pots) =
                self.encode_subchunk(section, section_y, overrides.get(&section_y))?;
            pots.extend(section_pots.into_iter().map(|(index, plant)| {
                let (x, z) = ((index % 16) as i32, (index / 16 % 16) as i32);
                let y = i32::from(section_y) * 16 + (index / 256) as i32;
                ((abs_chunk_x * 16 + x, y, abs_chunk_z * 16 + z), plant)
            }));
            records.push((
                build_chunk_key_bytes(
                    chunk_pos,
                    Dimension::Overworld,
                    KeyTypeTag::SubChunkPrefix,
                    Some(section_y),
                ),
                bytes,
            ));
        }

        // No entity record: the Java entities other than frames have no Bedrock form.
        let mut block_entities = Vec::new();
        if let Some(Value::List(list)) = chunk.other.get("block_entities") {
            for be in dedup_compound_list(list) {
                let Value::Compound(be) = be else { continue };
                let block = get_entity_coords(&be)
                    .and_then(|(x, y, z)| chunk.get_block((x & 15) as u8, y, (z & 15) as u8));
                block_entities.extend(bedrock_entities::block_entity(&be, block));
            }
        }
        block_entities.extend(frames.into_iter().map(|(_, _, be)| be));
        block_entities.extend(
            pots.into_iter()
                .map(|(pos, plant)| bedrock_entities::flower_pot(pos, &plant)),
        );
        if !block_entities.is_empty() {
            // Concatenated compounds, no list around them.
            let mut data: Vec<u8> = Vec::new();
            for compound in &mut block_entities {
                strip_empty_lists(compound);
                let bytes = nbtx::to_le_bytes(&*compound)
                    .map_err(|e| BedrockSaveError::Nbt(format!("block entity compound: {e}")))?;
                data.extend_from_slice(&bytes);
            }
            records.push((
                build_chunk_key_bytes(
                    chunk_pos,
                    Dimension::Overworld,
                    KeyTypeTag::BlockEntity,
                    None,
                ),
                data,
            ));
        }

        Ok(records)
    }

    /// A heightmap, then one biome storage per subchunk of the dimension from the bottom.
    fn create_data3d(&self, abs_chunk_x: i32, abs_chunk_z: i32, chunk: &ChunkToModify) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(512 + 64);
        for height in column_heights(chunk) {
            buffer.extend_from_slice(&height.to_le_bytes());
        }

        let origin = self.ground_origin.unwrap_or_default();
        let names = crate::biome::chunk_biome_names(
            abs_chunk_x,
            abs_chunk_z,
            self.ground.as_deref(),
            self.center_lat,
            origin,
        );
        // Biomes do not change with height, so the rest say "same as below".
        write_biome_storage(&mut buffer, &names.map(bedrock_biome_id));
        let (min_section, max_section) = self.section_span();
        buffer.extend(std::iter::repeat_n(
            0xFF,
            (max_section - min_section) as usize,
        ));
        buffer
    }

    /// The lowest and highest subchunk of the overworld, with the tall pack's bounds.
    fn section_span(&self) -> (i32, i32) {
        if self.extend_build_height {
            (-32, 31)
        } else {
            (-4, 19)
        }
    }

    /// Encode a section into Bedrock subchunk format
    fn encode_subchunk(
        &self,
        section: &SectionToModify,
        y_index: i8,
        overrides: Option<&StdHashMap<usize, BedrockBlock>>,
    ) -> Result<(Vec<u8>, PottedPlants), BedrockSaveError> {
        let mut buffer = Cursor::new(Vec::new());
        let section = self.build_palette_and_indices(section, overrides);

        // Format 9, one or two storage layers, then the Y index.
        buffer.write_u8(9)?;
        buffer.write_u8(if section.water.is_some() { 2 } else { 1 })?;
        buffer.write_i8(y_index)?;

        write_block_storage(&mut buffer, &section.palette, |i| {
            u32::from(section.indices[i])
        })?;
        // Water in the second layer, as Bedrock keeps a waterlogged block's water.
        if let Some(water) = &section.water {
            let palette = [
                BedrockBlock::simple("air"),
                BedrockBlock::with_states(
                    "water",
                    vec![("liquid_depth", BedrockBlockStateValue::Int(0))],
                ),
            ];
            write_block_storage(&mut buffer, &palette, |i| u32::from(water[i]))?;
        }

        Ok((buffer.into_inner(), section.pots))
    }

    /// Converts a section to Bedrock's XZY order: internal index y*256 + z*16 + x, Bedrock
    /// index x*256 + z*16 + y.
    fn build_palette_and_indices(
        &self,
        section: &SectionToModify,
        overrides: Option<&StdHashMap<usize, BedrockBlock>>,
    ) -> BedrockSection {
        let mut palette: Vec<BedrockBlock> = Vec::new();
        let mut palette_map: StdHashMap<BedrockBlock, u16> = StdHashMap::new();
        let mut indices = Box::new([0u16; 4096]);
        let mut water = Box::new([false; 4096]);
        let mut pots = Vec::new();

        // Air comes first, as the game writes it.
        palette_slot(&mut palette, &mut palette_map, BedrockBlock::simple("air"));

        // One block throughout, e.g. stone underground or the sky: converted once.
        if let BlockStorage::Uniform(block) = section.storage {
            if section.properties.is_empty() && overrides.is_none() && potted_plant(block).is_none()
            {
                let idx = palette_slot(
                    &mut palette,
                    &mut palette_map,
                    to_bedrock_block_with_properties(block, None),
                );
                let wet = is_waterlogged(block, None);
                return BedrockSection {
                    palette,
                    indices: Box::new([idx; 4096]),
                    water: wet.then(|| Box::new([true; 4096])),
                    pots,
                };
            }
        }

        // Identical property compounds share one Arc, so its address stands for the compound.
        let mut converted: StdHashMap<(Block, usize), (u16, bool, bool)> = StdHashMap::new();
        converted.insert((AIR, 0), (0, false, false));
        for x in 0..16usize {
            for z in 0..16usize {
                for y in 0..16usize {
                    let internal_idx = y * 256 + z * 16 + x;
                    let bedrock_idx = x * 256 + z * 16 + y;
                    if let Some(frame) = overrides.and_then(|o| o.get(&internal_idx)) {
                        indices[bedrock_idx] =
                            palette_slot(&mut palette, &mut palette_map, frame.clone());
                        continue;
                    }
                    let block = section.get_block_at_index(internal_idx);
                    let properties = section.properties.get(&internal_idx);
                    let key = (block, properties.map_or(0, |p| Arc::as_ptr(p) as usize));
                    let (idx, wet, potted) = *converted.entry(key).or_insert_with(|| {
                        let properties = properties.map(|p| p.as_ref());
                        let bedrock_block = to_bedrock_block_with_properties(block, properties);
                        (
                            palette_slot(&mut palette, &mut palette_map, bedrock_block),
                            is_waterlogged(block, properties),
                            potted_plant(block).is_some(),
                        )
                    });
                    indices[bedrock_idx] = idx;
                    water[bedrock_idx] = wet;
                    if potted {
                        pots.extend(potted_plant(block).map(|plant| (internal_idx, plant)));
                    }
                }
            }
        }

        let any_water = water.iter().any(|&w| w);
        BedrockSection {
            palette,
            indices,
            water: any_water.then_some(water),
            pots,
        }
    }

    fn write_metadata(
        &self,
        dir: &Path,
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
                projection: self.projection.clone(),
                scale: self.scale,
            },
            format: "bedrock-mcworld",
            chunk_count,
        };

        let metadata_bytes = serde_json::to_vec_pretty(&metadata)?;
        let metadata_path = dir.join("metadata.json");
        let mut file = File::create(metadata_path)?;
        file.write_all(&metadata_bytes)?;
        Ok(())
    }

    /// Packages the staging folder beside the archive and renames it into place, so a
    /// failure leaves the previous archive of the same name as it was.
    fn package_mcworld(&self, dir: &Path) -> Result<(), BedrockSaveError> {
        let mut part = self.mcworld_path.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let result = self
            .write_mcworld_archive(dir, &part)
            .and_then(|()| Ok(retry_transient(|| fs::rename(&part, &self.mcworld_path))?));
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result
    }

    fn write_mcworld_archive(&self, dir: &Path, archive: &Path) -> Result<(), BedrockSaveError> {
        let file = File::create(archive)?;
        let mut writer = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(CompressionMethod::Deflated);

        // Add top-level files
        for file_name in ["levelname.txt", "metadata.json", "level.dat"] {
            let path = dir.join(file_name);
            if path.exists() {
                writer.start_file(file_name, options)?;
                let contents = retry_transient(|| fs::read(&path))?;
                writer.write_all(&contents)?;
            }
        }

        // Add world_icon.jpeg from embedded assets
        const WORLD_ICON: &[u8] = include_bytes!("../../assets/minecraft/world_icon.jpeg");
        writer.start_file("world_icon.jpeg", options)?;
        writer.write_all(WORLD_ICON)?;

        let mut behavior_packs: Vec<&str> = Vec::new();
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
            behavior_packs.push(BP_HEADER_UUID);
        }

        let mut resource_packs: Vec<&str> = Vec::new();
        if let Some(addon) = &self.addon {
            // Folders get entries of their own, as the tall pack's do.
            let mut folders: BTreeSet<String> = BTreeSet::new();
            for (root, pack) in [
                ("behavior_packs", &addon.behavior),
                ("resource_packs", &addon.resources),
            ] {
                for (path, _) in &pack.files {
                    let full = format!("{root}/{}/{path}", pack.folder);
                    let mut end = 0;
                    while let Some(slash) = full[end..].find('/') {
                        end += slash + 1;
                        folders.insert(full[..end].to_string());
                    }
                }
            }
            if self.extend_build_height {
                folders.remove("behavior_packs/");
            }
            for folder in &folders {
                writer.add_directory(folder.as_str(), options)?;
            }
            for (root, pack) in [
                ("behavior_packs", &addon.behavior),
                ("resource_packs", &addon.resources),
            ] {
                for (path, bytes) in &pack.files {
                    writer.start_file(format!("{root}/{}/{path}", pack.folder), options)?;
                    writer.write_all(bytes)?;
                }
            }
            behavior_packs.push(&addon.behavior.id);
            resource_packs.push(&addon.resources.id);
        }
        for (file, ids) in [
            ("world_behavior_packs.json", &behavior_packs),
            ("world_resource_packs.json", &resource_packs),
        ] {
            if ids.is_empty() {
                continue;
            }
            let list: Vec<String> = ids
                .iter()
                .map(|id| format!(r#"{{"pack_id":"{id}","version":[1,0,0]}}"#))
                .collect();
            writer.start_file(file, options)?;
            writer.write_all(format!("[{}]", list.join(",")).as_bytes())?;
        }

        // Add db directory and its contents
        let db_path = dir.join("db");
        if db_path.is_dir() {
            add_directory_to_zip(&mut writer, &db_path, "db", options)?;
        }

        writer.finish()?.sync_all()?;
        Ok(())
    }
}

/// Appends the extension rather than replacing it, level names can contain dots.
fn append_mcworld_extension(dir: &std::path::Path) -> PathBuf {
    let mut name = dir.as_os_str().to_os_string();
    name.push(".mcworld");
    PathBuf::from(name)
}

/// Whether a directory is empty or ours. Only the marker counts, real worlds hold the same files.
fn is_arnis_staging_dir(dir: &std::path::Path) -> bool {
    if dir.join(STAGING_MARKER).is_file() {
        return true;
    }
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
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
            let contents = retry_transient(|| fs::read(&path))?;
            writer.write_all(&contents)?;
        } else if path.is_dir() {
            add_directory_to_zip(writer, &path, &zip_path, options)?;
        }
    }

    Ok(())
}

/// One above the highest block of each column, x fastest, 0 when empty, as Chunker writes it.
fn column_heights(chunk: &ChunkToModify) -> [i16; 256] {
    let mut heights = [0i16; 256];
    let mut found = [false; 256];
    let mut open = 256;
    let sections: Vec<_> = chunk.sections.iter().collect();
    for (section_y, section) in sections.into_iter().rev() {
        if matches!(section.storage, BlockStorage::Uniform(AIR)) {
            continue;
        }
        for column in 0..256usize {
            if found[column] {
                continue;
            }
            let (x, z) = ((column % 16) as u8, (column / 16) as u8);
            if let Some(y) = (0..16u8)
                .rev()
                .find(|&y| section.get_block(x, y, z).is_some())
            {
                heights[column] = (i32::from(section_y) * 16 + i32::from(y) + 1) as i16;
                found[column] = true;
                open -= 1;
            }
        }
        if open == 0 {
            break;
        }
    }
    heights
}

/// One subchunk's biomes from the chunk's 4x4 column grid, in the runtime form the game writes.
fn write_biome_storage(buffer: &mut Vec<u8>, ids: &[i32; 16]) {
    let mut palette: Vec<i32> = Vec::with_capacity(4);
    let mut grid = [0u32; 16];
    for (cell, id) in ids.iter().enumerate() {
        grid[cell] = match palette.iter().position(|p| p == id) {
            Some(i) => i as u32,
            None => {
                palette.push(*id);
                palette.len() as u32 - 1
            }
        };
    }
    if palette.len() == 1 {
        buffer.push(1);
        buffer.extend_from_slice(&palette[0].to_le_bytes());
        return;
    }
    let bits = bedrock_bits_per_block(palette.len() as u32);
    buffer.push((bits << 1) | 1);
    // XZY order; the grid cell is (z / 4) * 4 + x / 4.
    write_packed_indices(buffer, bits, |i| grid[(i >> 4 & 15) / 4 * 4 + (i >> 8) / 4])
        .expect("writing to a Vec cannot fail");
    buffer.extend_from_slice(&(palette.len() as i32).to_le_bytes());
    for id in palette {
        buffer.extend_from_slice(&id.to_le_bytes());
    }
}

/// Bedrock's numeric id of a Java biome; plains for one this table lacks.
fn bedrock_biome_id(java: &str) -> i32 {
    match java.trim_start_matches("minecraft:") {
        "ocean" => 0,
        "desert" => 2,
        "windswept_hills" => 3,
        "forest" => 4,
        "taiga" => 5,
        "swamp" => 6,
        "river" => 7,
        "frozen_river" => 11,
        "snowy_plains" => 12,
        "mushroom_fields" => 14,
        "beach" => 16,
        "jungle" => 21,
        "sparse_jungle" => 23,
        "deep_ocean" => 24,
        "stony_shore" => 25,
        "snowy_beach" => 26,
        "birch_forest" => 27,
        "dark_forest" => 29,
        "snowy_taiga" => 30,
        "old_growth_pine_taiga" => 32,
        "windswept_forest" => 34,
        "savanna" => 35,
        "savanna_plateau" => 36,
        "badlands" => 37,
        "wooded_badlands" => 38,
        "warm_ocean" => 40,
        "lukewarm_ocean" => 42,
        "deep_lukewarm_ocean" => 43,
        "cold_ocean" => 44,
        "deep_cold_ocean" => 45,
        "frozen_ocean" => 46,
        "deep_frozen_ocean" => 47,
        "bamboo_jungle" => 48,
        "sunflower_plains" => 129,
        "windswept_gravelly_hills" => 131,
        "flower_forest" => 132,
        "ice_spikes" => 140,
        "old_growth_birch_forest" => 155,
        "old_growth_spruce_taiga" => 160,
        "windswept_savanna" => 163,
        "eroded_badlands" => 165,
        "jagged_peaks" => 182,
        "frozen_peaks" => 183,
        "snowy_slopes" => 184,
        "grove" => 185,
        "meadow" => 186,
        "lush_caves" => 187,
        "dripstone_caves" => 188,
        "stony_peaks" => 189,
        "deep_dark" => 190,
        "mangrove_swamp" => 191,
        "cherry_grove" => 192,
        "pale_garden" => 193,
        _ => 1,
    }
}

/// One block storage layer: the packed indices, then the palette as NBT states.
fn write_block_storage(
    buffer: &mut impl IoWrite,
    palette: &[BedrockBlock],
    index: impl Fn(usize) -> u32,
) -> Result<(), BedrockSaveError> {
    let bits_per_block = bedrock_bits_per_block(palette.len() as u32);
    buffer.write_u8(bits_per_block << 1)?;
    write_packed_indices(buffer, bits_per_block, index)?;
    buffer.write_u32::<LittleEndian>(palette.len() as u32)?;
    for block in palette {
        let state = BedrockBlockState {
            name: block.name.clone(),
            states: block
                .states
                .iter()
                .map(|(k, v)| (k.clone(), Value::from(v)))
                .collect(),
            version: BLOCK_STATE_VERSION,
        };
        let nbt_bytes = nbtx::to_le_bytes(&state).map_err(|e| {
            BedrockSaveError::Nbt(format!("block palette state ({}): {e}", state.name))
        })?;
        buffer.write_all(&nbt_bytes)?;
    }
    Ok(())
}

/// The palette index of `block`, adding it when new.
fn palette_slot(
    palette: &mut Vec<BedrockBlock>,
    palette_map: &mut StdHashMap<BedrockBlock, u16>,
    block: BedrockBlock,
) -> u16 {
    if let Some(&idx) = palette_map.get(&block) {
        return idx;
    }
    let idx = palette.len() as u16;
    palette_map.insert(block.clone(), idx);
    palette.push(block);
    idx
}

/// Packs 4096 palette indices into words, `32 / bits` to a word and none split across two.
fn write_packed_indices(
    buffer: &mut impl IoWrite,
    bits_per_block: u8,
    index: impl Fn(usize) -> u32,
) -> std::io::Result<()> {
    let blocks_per_word = 32 / u32::from(bits_per_block);
    let mask = (1u32 << bits_per_block) - 1;
    for word_start in (0..4096).step_by(blocks_per_word as usize) {
        let mut word = 0u32;
        for (slot, i) in (word_start..(word_start + blocks_per_word as usize).min(4096)).enumerate()
        {
            word |= (index(i) & mask) << (u32::from(bits_per_block) * slot as u32);
        }
        buffer.write_u32::<LittleEndian>(word)?;
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

/// Leaves out the compound's empty lists, at any depth. nbtx writes an empty list
/// without its element type and length, which leaves every tag after it
/// unreadable. Bedrock reads a missing list as an empty one.
fn strip_empty_lists(compound: &mut StdHashMap<String, Value>) {
    // False for a list that is or holds an empty list. One inside a list can't be left
    // out without shifting the others, so the whole outer list goes.
    fn keep(value: &mut Value) -> bool {
        match value {
            Value::Compound(compound) => {
                strip_empty_lists(compound);
                true
            }
            Value::List(items) => !items.is_empty() && items.iter_mut().all(keep),
            _ => true,
        }
    }
    compound.retain(|_, value| keep(value));
}

/// The compounds of an NBT list, or nothing when it is absent or not a list.
fn compound_list(value: Option<&Value>) -> impl Iterator<Item = &StdHashMap<String, Value>> {
    let items = match value {
        Some(Value::List(items)) => items.as_slice(),
        _ => &[],
    };
    items.iter().filter_map(|item| match item {
        Value::Compound(c) => Some(c),
        _ => None,
    })
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
                bedrock_entities::int(&pos[0]),
                bedrock_entities::int(&pos[1]),
                bedrock_entities::int(&pos[2]),
            ) {
                return Some((x, y, z));
            }
        }
    }

    let (Some(x), Some(y), Some(z)) = (
        entity.get("x").and_then(bedrock_entities::int),
        entity.get("y").and_then(bedrock_entities::int),
        entity.get("z").and_then(bedrock_entities::int),
    ) else {
        return None;
    };

    Some((x, y, z))
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

/// The experiments compound as the game writes it with none toggled.
#[derive(Serialize)]
struct BedrockExperiments {
    experiments_ever_used: bool,
    saved_with_toggled_experiments: bool,
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

    fn test_writer(output_path: PathBuf) -> BedrockWriter {
        BedrockWriter::new(
            output_path,
            "Arnis World: Test".to_string(),
            None,
            None,
            false,
            "local".to_string(),
            1.0,
            crate::args::GameMode::Creative,
            0,
            false,
        )
    }

    #[test]
    fn mcworld_path_keeps_dots_in_area_name() {
        let output = PathBuf::from("Arnis St. Louis.mcworld");
        let writer = test_writer(output.clone());
        assert_eq!(writer.mcworld_path, output);
        assert_eq!(
            writer.beside_archive_staging_dir(),
            PathBuf::from("Arnis St. Louis.mcworld.staging")
        );
    }

    #[test]
    fn a_world_unpacked_beside_the_archive_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let unpacked = tmp.path().join("Arnis Test");
        fs::create_dir_all(unpacked.join("db")).unwrap();
        fs::write(unpacked.join("levelname.txt"), b"Arnis World: Test").unwrap();

        let writer = test_writer(unpacked.with_extension("mcworld"));
        writer.remove_legacy_staging();
        let staging = writer.staging_beside_archive().unwrap();
        assert!(staging.dir.join(STAGING_MARKER).is_file());
        assert!(unpacked.join("levelname.txt").is_file());
    }

    #[test]
    fn staging_left_by_an_earlier_version_is_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("Arnis Test");
        fs::create_dir_all(legacy.join("db")).unwrap();
        fs::write(legacy.join(STAGING_MARKER), b"").unwrap();
        let writer = test_writer(legacy.with_extension("mcworld"));
        let beside = writer.beside_archive_staging_dir();
        fs::create_dir_all(beside.join("db")).unwrap();
        fs::write(beside.join(STAGING_MARKER), b"").unwrap();

        writer.remove_legacy_staging();
        assert!(!legacy.exists());
        assert!(!beside.exists());
    }

    #[test]
    fn staging_beside_archive_keeps_foreign_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        let staging = writer.beside_archive_staging_dir();
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("notes.txt"), b"not ours").unwrap();

        assert!(writer.staging_beside_archive().is_err());
        assert!(staging.join("notes.txt").is_file());
    }

    #[test]
    fn staging_beside_archive_keeps_existing_bedrock_world() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        let staging = writer.beside_archive_staging_dir();
        fs::create_dir_all(staging.join("db")).unwrap();
        fs::write(staging.join("levelname.txt"), b"Someone's World").unwrap();
        fs::write(staging.join("db").join("CURRENT"), b"leveldb").unwrap();

        assert!(writer.staging_beside_archive().is_err());
        assert!(staging.join("db").join("CURRENT").is_file());
    }

    #[test]
    fn staging_beside_archive_wipes_previous_staging_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        let dir = writer.beside_archive_staging_dir();
        fs::create_dir_all(dir.join("db")).unwrap();
        fs::write(dir.join(STAGING_MARKER), b"").unwrap();
        fs::write(dir.join("levelname.txt"), b"Arnis World: Old").unwrap();

        let staging = writer.staging_beside_archive().unwrap();
        assert!(staging.dir.join(STAGING_MARKER).is_file());
        assert!(!staging.dir.join("levelname.txt").exists());
        drop(staging);
        assert!(!dir.exists());
    }

    #[test]
    fn staging_beside_archive_accepts_empty_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        fs::create_dir_all(writer.beside_archive_staging_dir()).unwrap();

        let staging = writer.staging_beside_archive().unwrap();
        assert!(staging.dir.join(STAGING_MARKER).is_file());
    }

    #[test]
    fn a_foreign_folder_beside_the_archive_does_not_stop_the_save() {
        let tmp = tempfile::tempdir().unwrap();
        let mut writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        let foreign = writer.beside_archive_staging_dir();
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("notes.txt"), b"not ours").unwrap();

        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        writer
            .write_world(&WorldToModify::default(), &xzbbox, &llbbox)
            .unwrap();
        assert!(writer.mcworld_path.is_file());
        assert!(foreign.join("notes.txt").is_file());
    }

    #[test]
    fn a_world_too_large_for_the_cache_drive_is_staged_beside_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        // A volume that reports no free space can't be compared against.
        if !fs2::available_space(std::env::temp_dir()).is_ok_and(|free| free > 0) {
            return;
        }
        let writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        let xzbbox = XZBBox::rect_from_xz_lengths(1e8, 1e8).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let staging = writer
            .stage_world(&WorldToModify::default(), &xzbbox, &llbbox)
            .unwrap();
        assert_eq!(staging.dir, writer.beside_archive_staging_dir());
    }

    #[test]
    fn a_failed_save_keeps_what_was_at_the_archive_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut writer = test_writer(tmp.path().join("Arnis Test.mcworld"));
        // A folder where the archive goes cannot be replaced by a file.
        fs::create_dir_all(writer.mcworld_path.join("keep")).unwrap();

        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        assert!(writer
            .write_world(&WorldToModify::default(), &xzbbox, &llbbox)
            .is_err());
        assert!(writer.mcworld_path.join("keep").is_dir());
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            leftovers,
            vec![std::ffi::OsString::from("Arnis Test.mcworld")]
        );
    }

    #[test]
    fn transient_errors_are_retried_and_lasting_ones_are_not() {
        let mut calls = 0;
        let result = retry_while(
            || {
                calls += 1;
                if calls < 3 {
                    Err("held")
                } else {
                    Ok(calls)
                }
            },
            |e| *e == "held",
        );
        assert_eq!(result.unwrap(), 3);

        let mut calls = 0;
        let result: Result<(), &str> = retry_while(
            || {
                calls += 1;
                Err("gone")
            },
            |e| *e == "held",
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);

        // A sharing violation is a hold on Windows; elsewhere nothing is.
        let sharing = std::io::Error::from_raw_os_error(32);
        assert_eq!(is_transient_io_error(&sharing), cfg!(windows));
        assert!(!is_transient_io_error(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
    }

    #[cfg(unix)]
    #[test]
    fn only_stale_marked_staging_folders_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
        let make = |name: &str, marked: bool, stale: bool| {
            let dir = root.path().join(name);
            fs::create_dir_all(&dir).unwrap();
            if marked {
                fs::write(dir.join(STAGING_MARKER), b"").unwrap();
            }
            if stale {
                File::open(&dir).unwrap().set_modified(old).unwrap();
            }
            dir
        };
        let stale = make("world-stale", true, true);
        let fresh = make("world-fresh", true, false);
        let foreign = make("someone-else", false, true);

        remove_stale_staging(root.path());
        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(foreign.exists());
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
            "local".to_string(),
            1.0,
            crate::args::GameMode::Creative,
            6000,
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
            "local".to_string(),
            1.0,
            crate::args::GameMode::Creative,
            6000,
            false,
        )
        .write_world(&world, &xzbbox, &llbbox)
        .expect("write_world");

        // Verify the mcworld was created
        let mcworld_path = output_dir.with_extension("mcworld");
        assert!(mcworld_path.exists(), "mcworld file should exist");
    }

    #[test]
    fn mob_spawning_sets_the_game_rules_and_the_difficulty_monsters_need() {
        use crate::args::MobSpawning;
        use std::io::Read;
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let world = WorldToModify::default();
        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();

        let written = |mobs: MobSpawning| {
            let output_dir = temp_dir.path().join(format!("{mobs:?}"));
            test_writer(output_dir.clone())
                .with_mob_spawning(mobs)
                .write_world(&world, &xzbbox, &llbbox)
                .expect("write_world");
            let file = fs::File::open(output_dir.with_extension("mcworld")).unwrap();
            let mut archive = ZipArchive::new(file).unwrap();
            let mut raw = Vec::new();
            archive
                .by_name("level.dat")
                .unwrap()
                .read_to_end(&mut raw)
                .unwrap();
            let mut nbt = &raw[8..];
            let level: StdHashMap<String, nbtx::Value> = nbtx::from_le_bytes(&mut nbt).unwrap();
            let flag = |key: &str| match level.get(key) {
                Some(nbtx::Value::Byte(b)) => *b != 0,
                other => panic!("{key}: {other:?}"),
            };
            let difficulty = match level.get("Difficulty") {
                Some(nbtx::Value::Int(d)) => *d,
                other => panic!("Difficulty: {other:?}"),
            };
            (
                flag("domobspawning"),
                flag("spawnMobs"),
                flag("mobgriefing"),
                difficulty,
            )
        };

        assert_eq!(written(MobSpawning::Off), (false, false, true, 2));
        assert_eq!(written(MobSpawning::Animals), (true, true, true, 0));
        assert_eq!(written(MobSpawning::All), (true, true, false, 2));
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
            "local".to_string(),
            1.0,
            crate::args::GameMode::Creative,
            6000,
            false,
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

    #[test]
    fn writes_the_vehicle_packs_beside_the_tall_pack_and_lists_both() {
        use crate::bedrock_addon::{Addon, Pack};
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let output_dir = temp_dir.path().join("bedrock_world_vehicles");
        let world = WorldToModify::default();
        let xzbbox = XZBBox::rect_from_xz_lengths(15.0, 15.0).unwrap();
        let llbbox = LLBBox::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let pack = |id: &str, files: &[&str]| Pack {
            id: id.to_string(),
            folder: "arnis_vehicles",
            files: files
                .iter()
                .map(|f| (f.to_string(), b"{}".to_vec()))
                .collect(),
        };
        let addon = Addon {
            behavior: pack("bp-id", &["manifest.json", "scripts/main.js"]),
            resources: pack("rp-id", &["manifest.json", "models/entity/car.geo.json"]),
        };

        BedrockWriter::new(
            output_dir.clone(),
            "vehicle-world".to_string(),
            None,
            None,
            true,
            "local".to_string(),
            1.0,
            crate::args::GameMode::Creative,
            6000,
            false,
        )
        .with_addon(Some(addon))
        .write_world(&world, &xzbbox, &llbbox)
        .expect("write_world");

        let file = fs::File::open(output_dir.with_extension("mcworld")).expect("archive");
        let mut archive = ZipArchive::new(file).expect("zip readable");
        let entries: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        let mut unique = entries.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), entries.len(), "no entry twice: {entries:?}");
        for path in [
            "behavior_packs/arnis_vehicles/",
            "behavior_packs/arnis_vehicles/scripts/",
            "behavior_packs/arnis_vehicles/scripts/main.js",
            "resource_packs/",
            "resource_packs/arnis_vehicles/models/entity/car.geo.json",
            "behavior_packs/arnis_tall/manifest.json",
        ] {
            assert!(
                entries.iter().any(|e| e == path),
                "missing {path}: {entries:?}"
            );
        }
        let mut read = |name: &str| -> Value {
            let mut f = archive.by_name(name).expect(name);
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
            serde_json::from_slice(&buf).unwrap()
        };
        let behavior = read("world_behavior_packs.json");
        let ids: Vec<&str> = behavior
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["pack_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["a7f3b2e0-8c4d-4e92-9b1a-3d7f5c8e4a61", "bp-id"]);
        assert_eq!(read("world_resource_packs.json")[0]["pack_id"], "rp-id");
    }

    /// Every compound of a record that holds several back to back.
    fn compounds(mut bytes: &[u8]) -> Vec<StdHashMap<String, nbtx::Value>> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            out.push(nbtx::from_le_bytes(&mut bytes).expect("compound"));
        }
        out
    }

    fn nbt_str(value: Option<&nbtx::Value>) -> Option<&str> {
        match value {
            Some(nbtx::Value::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// A chunk with a block, a potted plant, a chest and an item frame, read back.
    #[test]
    fn a_chunk_is_written_in_the_current_format() {
        use fastnbt::Value as V;
        let mut world = WorldToModify::default();
        world.set_block_if_absent(2, 64, 3, crate::block_definitions::STONE);
        world.set_block_if_absent(5, 64, 5, crate::block_definitions::POTTED_RED_TULIP);
        world.set_block_if_absent(6, 64, 5, crate::block_definitions::CHEST);
        let chunk = world.get_or_create_region(0, 0).get_or_create_chunk(0, 0);
        let item = StdHashMap::from([
            ("id".to_string(), V::String("minecraft:apple".into())),
            ("count".to_string(), V::Int(2)),
            ("Slot".to_string(), V::Byte(0)),
        ]);
        let chest = StdHashMap::from([
            ("id".to_string(), V::String("minecraft:chest".into())),
            ("x".to_string(), V::Int(6)),
            ("y".to_string(), V::Int(64)),
            ("z".to_string(), V::Int(5)),
            ("Items".to_string(), V::List(vec![V::Compound(item)])),
        ]);
        chunk.other.insert(
            "block_entities".to_string(),
            V::List(vec![V::Compound(chest)]),
        );
        let frame = StdHashMap::from([
            ("id".to_string(), V::String("minecraft:item_frame".into())),
            ("TileX".to_string(), V::Int(7)),
            ("TileY".to_string(), V::Int(65)),
            ("TileZ".to_string(), V::Int(5)),
            ("Facing".to_string(), V::Byte(3)),
        ]);
        let display = StdHashMap::from([(
            "id".to_string(),
            V::String("minecraft:block_display".into()),
        )]);
        chunk.other.insert(
            "entities".to_string(),
            V::List(vec![V::Compound(frame), V::Compound(display)]),
        );

        let writer = test_writer(PathBuf::from("unused.mcworld"));
        let records: StdHashMap<Vec<u8>, Vec<u8>> = writer
            .encode_chunk_records(
                0,
                0,
                world.get_or_create_region(0, 0).get_or_create_chunk(0, 0),
            )
            .unwrap()
            .into_iter()
            .collect();
        let pos = Vec2::new(0, 0);
        let record = |tag, y| {
            records
                .get(&build_chunk_key_bytes(pos, Dimension::Overworld, tag, y))
                .cloned()
        };

        assert_eq!(record(KeyTypeTag::Version, None), Some(vec![CHUNK_VERSION]));
        assert_eq!(
            record(KeyTypeTag::FinalizedState, None),
            Some(2i32.to_le_bytes().to_vec())
        );
        assert!(record(KeyTypeTag::Entity, None).is_none());

        // Heightmap, then one plains storage and 23 "same as below".
        let data3d = record(KeyTypeTag::Data3D, None).unwrap();
        assert_eq!(data3d.len(), 512 + 5 + 23);
        assert_eq!(i16::from_le_bytes([data3d[100], data3d[101]]), 65);
        assert_eq!(&data3d[512..517], &[1, 1, 0, 0, 0]);
        assert!(data3d[517..].iter().all(|&b| b == 0xFF));

        // Section 4: air, stone, pot, chest and frame, so 3 bits, 10 to a word.
        let sub = record(KeyTypeTag::SubChunkPrefix, Some(4)).unwrap();
        assert_eq!(&sub[..4], &[9, 1, 4, 3 << 1]);
        let count_at = 4 + 4096usize.div_ceil(10) * 4;
        let count = u32::from_le_bytes(sub[count_at..count_at + 4].try_into().unwrap());
        let palette = compounds(&sub[count_at + 4..]);
        assert_eq!(palette.len(), count as usize);
        assert!(palette.iter().all(
            |e| matches!(e.get("version"), Some(nbtx::Value::Int(v)) if *v == BLOCK_STATE_VERSION)
        ));
        let names: Vec<_> = palette
            .iter()
            .filter_map(|e| nbt_str(e.get("name")))
            .collect();
        assert!(names.contains(&"minecraft:frame"), "{names:?}");
        assert!(names.contains(&"minecraft:flower_pot"), "{names:?}");

        let entities = compounds(&record(KeyTypeTag::BlockEntity, None).unwrap());
        let ids: Vec<_> = entities
            .iter()
            .filter_map(|e| nbt_str(e.get("id")))
            .collect();
        assert_eq!(ids.len(), 3, "{ids:?}");
        for id in ["Chest", "ItemFrame", "FlowerPot"] {
            assert!(ids.contains(&id), "{ids:?}");
        }
    }

    #[test]
    fn empty_lists_are_left_out_at_any_depth() {
        use fastnbt::Value as V;
        let mut compound = StdHashMap::from([
            ("lists".to_string(), V::List(vec![V::List(Vec::new())])),
            (
                "items".to_string(),
                V::List(vec![V::Compound(StdHashMap::from([
                    ("tags".to_string(), V::List(Vec::new())),
                    ("count".to_string(), V::Int(1)),
                ]))]),
            ),
            ("x".to_string(), V::Int(4)),
        ]);
        strip_empty_lists(&mut compound);
        assert!(!compound.contains_key("lists"));
        assert_eq!(compound.get("x"), Some(&V::Int(4)));
        let Some(V::List(items)) = compound.get("items") else {
            panic!("items kept");
        };
        let V::Compound(item) = &items[0] else {
            panic!("an item compound");
        };
        assert!(!item.contains_key("tags"));
        assert_eq!(item.get("count"), Some(&V::Int(1)));
    }

    #[test]
    fn an_empty_chest_does_not_break_the_block_entities_after_it() {
        use fastnbt::Value as V;
        let mut world = WorldToModify::default();
        world.set_block_if_absent(1, 64, 1, crate::block_definitions::CHEST);
        world.set_block_if_absent(2, 64, 1, crate::block_definitions::CHEST);
        let chest = |x: i32, items: Vec<V>| {
            V::Compound(StdHashMap::from([
                ("id".to_string(), V::String("minecraft:chest".into())),
                ("x".to_string(), V::Int(x)),
                ("y".to_string(), V::Int(64)),
                ("z".to_string(), V::Int(1)),
                ("Items".to_string(), V::List(items)),
            ]))
        };
        let apple = V::Compound(StdHashMap::from([
            ("id".to_string(), V::String("minecraft:apple".into())),
            ("count".to_string(), V::Int(1)),
            ("Slot".to_string(), V::Byte(0)),
        ]));
        let chunk = world.get_or_create_region(0, 0).get_or_create_chunk(0, 0);
        chunk.other.insert(
            "block_entities".to_string(),
            V::List(vec![chest(1, Vec::new()), chest(2, vec![apple])]),
        );

        let writer = test_writer(PathBuf::from("unused.mcworld"));
        let records = writer
            .encode_chunk_records(
                0,
                0,
                world.get_or_create_region(0, 0).get_or_create_chunk(0, 0),
            )
            .unwrap();
        let key = build_chunk_key_bytes(
            Vec2::new(0, 0),
            Dimension::Overworld,
            KeyTypeTag::BlockEntity,
            None,
        );
        let (_, data) = records.iter().find(|(k, _)| *k == key).unwrap();
        let entities = compounds(data);
        assert_eq!(entities.len(), 2);
        for entity in &entities {
            assert_eq!(nbt_str(entity.get("id")), Some("Chest"));
            assert!(matches!(entity.get("Findable"), Some(nbtx::Value::Byte(0))));
        }
        let item_counts: Vec<usize> = entities
            .iter()
            .map(|e| match e.get("Items") {
                Some(nbtx::Value::List(items)) => items.len(),
                _ => 0,
            })
            .collect();
        assert!(
            item_counts.contains(&0) && item_counts.contains(&1),
            "{item_counts:?}"
        );
    }

    #[test]
    fn kelp_gets_water_in_the_second_layer_and_frames_skip_walls() {
        use fastnbt::Value as V;
        let mut world = WorldToModify::default();
        world.set_block_if_absent(1, 40, 1, crate::block_definitions::KELP);
        world.set_block_if_absent(4, 40, 4, crate::block_definitions::STONE);
        let chunk = world.get_or_create_region(0, 0).get_or_create_chunk(0, 0);
        let frame = StdHashMap::from([
            ("id".to_string(), V::String("minecraft:item_frame".into())),
            ("TileX".to_string(), V::Int(4)),
            ("TileY".to_string(), V::Int(40)),
            ("TileZ".to_string(), V::Int(4)),
        ]);
        chunk
            .other
            .insert("entities".to_string(), V::List(vec![V::Compound(frame)]));

        let writer = test_writer(PathBuf::from("unused.mcworld"));
        let chunk = world.get_or_create_region(0, 0).get_or_create_chunk(0, 0);
        let records: StdHashMap<Vec<u8>, Vec<u8>> = writer
            .encode_chunk_records(0, 0, chunk)
            .unwrap()
            .into_iter()
            .collect();
        let key = |tag, y| build_chunk_key_bytes(Vec2::new(0, 0), Dimension::Overworld, tag, y);

        // Air, kelp and stone in 2 bits, 16 to a word; then the water layer in 1 bit.
        let sub = &records[&key(KeyTypeTag::SubChunkPrefix, Some(2))];
        assert_eq!(&sub[..4], &[9, 2, 2, 2 << 1]);
        let mut rest = &sub[4 + 256 * 4..];
        let count = u32::from_le_bytes(rest[..4].try_into().unwrap());
        rest = &rest[4..];
        let blocks: Vec<StdHashMap<String, nbtx::Value>> = (0..count)
            .map(|_| nbtx::from_le_bytes(&mut rest).unwrap())
            .collect();
        assert!(!blocks
            .iter()
            .any(|b| nbt_str(b.get("name")) == Some("minecraft:frame")));
        assert_eq!(rest[0], 1 << 1);
        let water = compounds(&rest[1 + 128 * 4 + 4..]);
        let names: Vec<_> = water
            .iter()
            .filter_map(|e| nbt_str(e.get("name")))
            .collect();
        assert_eq!(names, ["minecraft:air", "minecraft:water"]);
        assert!(!records.contains_key(&key(KeyTypeTag::BlockEntity, None)));
    }

    #[test]
    fn mixed_biomes_are_packed_per_block() {
        let mut ids = [1i32; 16];
        ids[3] = 2; // x 12..16, z 0..4
        let mut buffer = Vec::new();
        write_biome_storage(&mut buffer, &ids);
        assert_eq!(buffer[0], (1 << 1) | 1);
        // 128 words, then the palette.
        assert_eq!(&buffer[1 + 512..], &[2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
        let bit = |i: usize| {
            let word = u32::from_le_bytes(
                buffer[1 + i / 32 * 4..1 + i / 32 * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
            word >> (i % 32) & 1
        };
        // XZY order: x 12, z 0 is desert; x 0, z 12 is plains.
        assert_eq!(bit(12 * 256), 1);
        assert_eq!(bit(12 * 16), 0);
    }

    /// Multi-chunk, multi-section world: exercises the parallel encode path and verifies the
    /// expected LevelDB records actually land (the empty-world tests above never reached encoding).
    #[test]
    fn parallel_chunk_encode_writes_expected_records() {
        let mut world = WorldToModify::default();
        // Blocks across several chunks spanning two regions, at three section heights (4, 0, -2).
        let chunk_coords = [(0i32, 0i32), (1, 0), (0, 1), (33, 2)];
        for &(cx, cz) in &chunk_coords {
            for y in [70i32, 5, -20] {
                world.set_block_if_absent(
                    cx * 16 + 2,
                    y,
                    cz * 16 + 3,
                    crate::block_definitions::STONE,
                );
            }
        }

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let writer = BedrockWriter::new(
            temp_dir.path().join("world"),
            "Parallel Test".to_string(),
            None,
            None,
            false,
            "local".to_string(),
            1.0,
            crate::args::GameMode::Creative,
            6000,
            false,
        );
        let staging = temp_dir.path().join("staging");
        fs::create_dir_all(&staging).expect("staging dir");
        writer
            .write_chunks_to_db(&staging, &world)
            .expect("write chunks");

        // Reopen the LevelDB and verify each populated chunk got version + Data3D + subchunk(4).
        let mut opts = bedrock_db_options();
        opts.create_if_missing = false;
        let mut db = DB::open(staging.join("db").into_boxed_path(), opts).expect("reopen db");

        for &(cx, cz) in &chunk_coords {
            let pos = Vec2::new(cx, cz);
            let version =
                build_chunk_key_bytes(pos, Dimension::Overworld, KeyTypeTag::Version, None);
            assert_eq!(
                db.get(&version).as_deref(),
                Some(&[42u8][..]),
                "version record missing for chunk ({cx}, {cz})"
            );

            let data3d = build_chunk_key_bytes(pos, Dimension::Overworld, KeyTypeTag::Data3D, None);
            assert!(
                db.get(&data3d).is_some(),
                "Data3D record missing for chunk ({cx}, {cz})"
            );

            // Every section we populated (positive, zero, negative index) must be written.
            for section_y in [4i8, 0, -2] {
                let subchunk = build_chunk_key_bytes(
                    pos,
                    Dimension::Overworld,
                    KeyTypeTag::SubChunkPrefix,
                    Some(section_y),
                );
                assert!(
                    db.get(&subchunk).is_some_and(|b| !b.is_empty()),
                    "subchunk {section_y} missing for chunk ({cx}, {cz})"
                );
            }
        }
    }
}

#[cfg(test)]
mod leveldb_options_tests {
    use super::*;

    /// Tiny memtables and tables force many flushes and compactions, and so many
    /// table-cache evictions. Under stock options with a small table cache this
    /// workload corrupts rusty-leveldb's LRU list and panics in `cache.rs`.
    #[test]
    fn bedrock_db_survives_heavy_compaction() {
        let tmp = tempfile::tempdir().unwrap();
        let mut opts = bedrock_db_options();
        opts.create_if_missing = true;
        opts.write_buffer_size = 16 * 1024;
        opts.max_file_size = 16 * 1024;
        let mut db = DB::open(tmp.path().join("db").into_boxed_path(), opts).unwrap();

        // xorshift, so keys land out of order and overlap across tables
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for i in 0..15_000u32 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let key = (x % 10_000).to_le_bytes();
            db.put(&key, &[(i % 251) as u8; 200]).unwrap();
        }
        db.flush().unwrap();
        assert!(db.get(&(x % 10_000).to_le_bytes()).is_some());
    }

    #[test]
    fn monotonic_env_clock_never_goes_backwards() {
        let env = MonotonicDiskEnv::new();
        let a = env.micros();
        assert!(a > 0);
        assert!(env.micros() >= a);
    }
}
