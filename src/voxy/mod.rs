//! Pre-generated LOD data for the [voxy](https://github.com/MCRcortex/voxy)
//! rendering mod (Java worlds only).
//!
//! Voxy normally builds this itself, either as you fly around or in one pass via
//! `/voxy import current`. Both read back the region files Arnis just wrote, so
//! the work is pure duplication: we already hold every block, its baked light
//! and its biome in memory at save time. This module writes voxy's database
//! directly instead, so a generated world renders to the horizon the first time
//! it is opened.
//!
//! The layout was derived from voxy's own sources and verified against a world
//! the mod imported itself:
//!
//! - `<save>/voxy/config.json` pins the storage backend for this world.
//! - `<save>/voxy/<world id>/storage/` is a RocksDB with a `world_sections` and
//!   an `id_mappings` column family. See [`rocks`] for how it is emitted
//!   without linking RocksDB.
//! - The world id is `sha256(biome_zoom_seed || dimension key)`, truncated to 32
//!   hex characters. See [`world_id`].

mod lod;
mod mapper;
mod rocks;

pub(crate) use lod::RegionLod;

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::world_editor::common::PaletteItem;
use mapper::Mapper;

/// Matches the `compressionLevel` in the config we write, and voxy's default.
const ZSTD_LEVEL: i32 = 1;

/// File numbers for the database we lay down. RocksDB only requires that
/// `NEXT_FILE_NUMBER` exceeds every number already on disk.
const WAL_FILE_NUMBER: u64 = 4;
const MANIFEST_FILE_NUMBER: u64 = 5;
const NEXT_FILE_NUMBER: u64 = 6;

/// The per-world storage config. Voxy rewrites this file on load, so it only
/// has to be valid; writing it pins the backend and compressor this module
/// assumes even if the user's defaults differ.
const CONFIG_JSON: &str = r#"{
  "version": 1,
  "disabled": false,
  "sectionStorageConfig": {
    "TYPE": "Serializer",
    "storage": {
      "TYPE": "CompressionAdaptor",
      "compressor": {
        "TYPE": "ZSTD",
        "compressionLevel": 1
      },
      "delegate": {
        "TYPE": "RocksDB"
      }
    }
  }
}"#;

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Minecraft's `BiomeManager.obfuscateSeed`: the first eight bytes of the
/// SHA-256 of the level seed, read little-endian. This is the value the `Level`
/// constructor receives, and the one voxy hashes into its world id.
fn obfuscate_seed(seed: i64) -> i64 {
    let digest = Sha256::digest(seed.to_le_bytes());
    i64::from_le_bytes(digest[..8].try_into().expect("sha256 is 32 bytes"))
}

/// Voxy's `WorldIdentifier.getWorldId` for the overworld: the obfuscated seed
/// concatenated with the dimension `ResourceKey`'s `toString`, hashed and cut to
/// 32 hex characters. Arnis only ever writes the overworld.
pub(crate) fn world_id(seed: i64) -> String {
    let data = format!(
        "{}ResourceKey[minecraft:dimension / minecraft:overworld]",
        obfuscate_seed(seed)
    );
    hex(&Sha256::digest(data.as_bytes()))[..32].to_string()
}

/// A stable database id, so regenerating a world twice produces identical files.
fn derive_db_id(world_id: &str) -> String {
    let digest = Sha256::digest(format!("arnis-voxy-db-id:{world_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant
    format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16])
    )
}

/// Reads `Data.WorldGenSettings.seed` out of a world's `level.dat`.
///
/// The seed decides the storage directory name, so a wrong one would leave voxy
/// looking at an empty database rather than anything broken.
fn read_world_seed(world_dir: &Path) -> Option<i64> {
    let raw = fs::read(world_dir.join("level.dat")).ok()?;
    let mut plain = Vec::new();
    let nbt = if flate2::read::GzDecoder::new(&raw[..])
        .read_to_end(&mut plain)
        .is_ok()
    {
        plain
    } else {
        raw
    };

    let value: fastnbt::Value = fastnbt::from_bytes(&nbt).ok()?;
    let fastnbt::Value::Compound(root) = value else {
        return None;
    };
    let fastnbt::Value::Compound(data) = root.get("Data")? else {
        return None;
    };
    let fastnbt::Value::Compound(settings) = data.get("WorldGenSettings")? else {
        return None;
    };
    match settings.get("seed")? {
        fastnbt::Value::Long(seed) => Some(*seed),
        fastnbt::Value::Int(seed) => Some(*seed as i64),
        _ => None,
    }
}

/// The write-ahead log under construction, plus the sequence counter RocksDB
/// replays it with.
struct Wal {
    log: rocks::LogWriter<BufWriter<File>>,
    seq: u64,
    error: Option<std::io::Error>,
    sections: u64,
    bytes: u64,
}

impl Wal {
    /// Appends one put. Errors are latched rather than propagated: the callers
    /// sit deep inside the region write loop, and a half-written LOD cache must
    /// not fail a world that is otherwise fine. [`VoxyWriter::finish`] reports.
    fn put(&mut self, cf: u32, key: &[u8], value: &[u8]) {
        if self.error.is_some() {
            return;
        }
        self.seq += 1;
        let batch = rocks::write_batch_put(self.seq, cf, key, value);
        if let Err(e) = self.log.add_record(&batch) {
            self.error = Some(e);
            return;
        }
        self.bytes += value.len() as u64;
    }
}

/// Collects a world's LOD sections and writes voxy's database.
///
/// Shared across the region-save threads and the background flush worker, so
/// every entry point takes `&self`.
pub struct VoxyWriter {
    storage_dir: PathBuf,
    config_dir: PathBuf,
    db_id: String,
    wal: Mutex<Wal>,
    mapper: Mutex<Mapper>,
}

impl VoxyWriter {
    /// Prepares the database directory for `world_dir`. Returns `None` when the
    /// world's seed cannot be read, since without it the data would land in a
    /// directory voxy never looks at.
    pub fn create(world_dir: &Path) -> Result<Option<Self>, std::io::Error> {
        let Some(seed) = read_world_seed(world_dir) else {
            return Ok(None);
        };
        let world_id = world_id(seed);
        let config_dir = world_dir.join("voxy");
        let storage_dir = config_dir.join(&world_id).join("storage");

        // A regenerated world must not inherit half of an older LOD cache.
        if storage_dir.exists() {
            fs::remove_dir_all(&storage_dir)?;
        }
        fs::create_dir_all(&storage_dir)?;

        let log = File::create(storage_dir.join(format!("{WAL_FILE_NUMBER:06}.log")))?;
        Ok(Some(Self {
            storage_dir,
            config_dir,
            db_id: derive_db_id(&world_id),
            wal: Mutex::new(Wal {
                log: rocks::LogWriter::new(BufWriter::with_capacity(1 << 20, log)),
                seq: 0,
                error: None,
                sections: 0,
                bytes: 0,
            }),
            mapper: Mutex::new(Mapper::new()),
        }))
    }

    /// A builder for one region's slice of the LOD pyramid.
    pub(crate) fn region_lod(&self, min_section_y: i32, max_section_y: i32) -> RegionLod<'_> {
        RegionLod::new(self, min_section_y, max_section_y)
    }

    /// Registers a blockstate, returning its voxy id and light dampening.
    pub(crate) fn intern_block(&self, key: &str, item: &PaletteItem) -> (u32, u8) {
        let mut mapper = self.mapper.lock().unwrap_or_else(|p| p.into_inner());
        let id = mapper.block_id(key, item);
        let opacity = mapper.opacity_table()[id as usize];
        (id, opacity)
    }

    pub(crate) fn intern_biome(&self, name: &str) -> u32 {
        self.mapper
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .biome_id(name)
    }

    /// Compresses and stores one serialized section. Compression happens before
    /// the lock so the region threads do not serialize on it.
    pub(crate) fn put_section(&self, key: i64, serialized: &[u8]) {
        let compressed = match zstd::bulk::compress(serialized, ZSTD_LEVEL) {
            Ok(bytes) => bytes,
            Err(e) => {
                let mut wal = self.wal.lock().unwrap_or_else(|p| p.into_inner());
                if wal.error.is_none() {
                    wal.error = Some(e);
                }
                return;
            }
        };
        let mut wal = self.wal.lock().unwrap_or_else(|p| p.into_inner());
        wal.put(rocks::CF_WORLD_SECTIONS, &key.to_be_bytes(), &compressed);
        wal.sections += 1;
    }

    /// Flushes the id registries into the log, then lays down the manifest that
    /// makes the directory a database RocksDB will open.
    pub fn finish(&self) -> Result<(u64, u64), std::io::Error> {
        let mut wal = self.wal.lock().unwrap_or_else(|p| p.into_inner());
        {
            let mapper = self.mapper.lock().unwrap_or_else(|p| p.into_inner());
            for record in mapper.records() {
                wal.put(
                    rocks::CF_ID_MAPPINGS,
                    &record.key.to_be_bytes(),
                    &record.body,
                );
            }
        }
        wal.log.flush()?;
        if let Some(e) = wal.error.take() {
            return Err(e);
        }

        fs::write(
            self.storage_dir
                .join(format!("MANIFEST-{MANIFEST_FILE_NUMBER:06}")),
            rocks::manifest_bytes(&self.db_id, WAL_FILE_NUMBER, NEXT_FILE_NUMBER),
        )?;
        fs::write(
            self.storage_dir.join("CURRENT"),
            format!("MANIFEST-{MANIFEST_FILE_NUMBER:06}\n"),
        )?;
        fs::write(self.storage_dir.join("IDENTITY"), &self.db_id)?;
        fs::write(self.config_dir.join("config.json"), CONFIG_JSON)?;

        Ok((wal.sections, wal.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference world: seed -1743857021154781143 produced this directory
    /// name, and voxy found its data there.
    #[test]
    fn world_id_matches_a_real_voxy_directory() {
        assert_eq!(obfuscate_seed(-1743857021154781143), -3706830175705209740);
        assert_eq!(
            world_id(-1743857021154781143),
            "5e5e936bbbed1f67aef5fc58f70f48a8"
        );
    }

    #[test]
    fn db_ids_are_uuid_shaped_and_stable() {
        let a = derive_db_id("5e5e936bbbed1f67aef5fc58f70f48a8");
        assert_eq!(a, derive_db_id("5e5e936bbbed1f67aef5fc58f70f48a8"));
        assert_ne!(a, derive_db_id("0000000000000000000000000000000f"));
        assert_eq!(a.len(), 36);
        assert_eq!(
            a.split('-').map(str::len).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert_eq!(&a[14..15], "4", "uuid version nibble");
    }

    #[test]
    fn missing_level_dat_disables_the_writer() {
        let dir = tempfile::tempdir().unwrap();
        assert!(VoxyWriter::create(dir.path()).unwrap().is_none());
    }

    /// End to end: a writer with one section produces a database directory with
    /// every file RocksDB needs to recover.
    #[test]
    fn finish_lays_down_a_complete_database() {
        let dir = tempfile::tempdir().unwrap();
        write_level_dat(dir.path(), -1743857021154781143);

        let writer = VoxyWriter::create(dir.path()).unwrap().unwrap();
        let key = lod::section_key(0, 1, 0, 2);
        writer.put_section(key, &lod::serialize_section(key, &vec![7u64; 32768], 0xFF));
        let (sections, _) = writer.finish().unwrap();
        assert_eq!(sections, 1);

        let storage = dir
            .path()
            .join("voxy")
            .join("5e5e936bbbed1f67aef5fc58f70f48a8")
            .join("storage");
        for name in ["CURRENT", "IDENTITY", "MANIFEST-000005", "000004.log"] {
            assert!(storage.join(name).is_file(), "missing {name}");
        }
        assert_eq!(
            fs::read_to_string(storage.join("CURRENT")).unwrap(),
            "MANIFEST-000005\n"
        );
        assert!(dir.path().join("voxy").join("config.json").is_file());

        // The put is recoverable: sequence 1, column family 1, big-endian key.
        let wal = fs::read(storage.join("000004.log")).unwrap();
        let payload = &wal[7..];
        assert_eq!(u64::from_le_bytes(payload[0..8].try_into().unwrap()), 1);
        assert_eq!(payload[12], 0x05);
        assert_eq!(payload[13], 1);
        assert_eq!(payload[14], 8);
        assert_eq!(&payload[15..23], &key.to_be_bytes());
    }

    /// Regenerating into the same folder replaces the cache instead of mixing
    /// a new WAL into an old database.
    #[test]
    fn regenerating_clears_a_stale_database() {
        let dir = tempfile::tempdir().unwrap();
        write_level_dat(dir.path(), -1743857021154781143);
        let storage = dir
            .path()
            .join("voxy")
            .join("5e5e936bbbed1f67aef5fc58f70f48a8")
            .join("storage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(storage.join("000008.sst"), b"stale").unwrap();

        let _writer = VoxyWriter::create(dir.path()).unwrap().unwrap();
        assert!(!storage.join("000008.sst").exists());
    }

    fn write_level_dat(dir: &Path, seed: i64) {
        use std::collections::HashMap;
        use std::io::Write;

        let settings = HashMap::from([("seed".to_string(), fastnbt::Value::Long(seed))]);
        let data = HashMap::from([(
            "WorldGenSettings".to_string(),
            fastnbt::Value::Compound(settings),
        )]);
        let root = HashMap::from([("Data".to_string(), fastnbt::Value::Compound(data))]);
        let bytes = fastnbt::to_bytes(&fastnbt::Value::Compound(root)).unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        fs::write(dir.join("level.dat"), encoder.finish().unwrap()).unwrap();
    }
}
