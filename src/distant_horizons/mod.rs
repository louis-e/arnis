//! Pre-generated LODs for the Distant Horizons mod (Java only), written straight into
//! `<save>/data/DistantHorizons.sqlite` so DH needn't rebuild them from the regions.
//! Tables follow DH's `sqlScripts/`, rows its `FullDataSourceV2DTO`.

mod encode;
mod lod;

pub(crate) use lod::RegionLod;

use rusqlite::{params, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const DB_NAME: &str = "DistantHorizons.sqlite";

/// The tables as DH's update scripts leave them.
const SCHEMA: &str = "
CREATE TABLE Schema(
    SchemaVersionId INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    ScriptName TEXT NOT NULL UNIQUE,
    AppliedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP
);
CREATE TABLE Legacy_FullData_V1(
    DhSectionPos TEXT NOT NULL PRIMARY KEY,
    DataDetailLevel TINYINT NULL,
    Checksum INT NULL,
    DataVersion BIGINT NULL,
    WorldGenStep NVARCHAR(32) NULL,
    DataType NVARCHAR(48) NULL,
    BinaryDataFormatVersion TINYINT NULL,
    Data BLOB NULL,
    CreatedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP,
    LastModifiedDateTime DATETIME NOT NULL default CURRENT_TIMESTAMP,
    MigrationFailed BIT NOT NULL DEFAULT 0
);
CREATE TABLE FullData(
    DetailLevel TINYINT NOT NULL,
    PosX INT NOT NULL,
    PosZ INT NOT NULL,
    MinY INT NOT NULL,
    DataChecksum INT NOT NULL,
    Data BLOB NULL,
    ColumnGenerationStep BLOB NULL,
    ColumnWorldCompressionMode BLOB NULL,
    Mapping BLOB NULL,
    DataFormatVersion TINYINT NULL,
    CompressionMode TINYINT NULL,
    ApplyToParent BIT NULL,
    LastModifiedUnixDateTime BIGINT NOT NULL,
    CreatedUnixDateTime BIGINT NOT NULL,
    ApplyToChildren BIT NULL,
    NorthAdjData BLOB NULL,
    SouthAdjData BLOB NULL,
    EastAdjData BLOB NULL,
    WestAdjData BLOB NULL,
    Regenerate BIT NULL,
    PRIMARY KEY (DetailLevel, PosX, PosZ)
);
CREATE INDEX FullDataUpdatedIndex ON FullData (ApplyToParent) WHERE ApplyToParent = 1;
CREATE INDEX FullDataApplyToChildrenIndex ON FullData (ApplyToChildren) WHERE ApplyToChildren = 1;
CREATE INDEX FullDataRegenerateIndex ON FullData (Regenerate) WHERE Regenerate = 1;
CREATE TABLE ChunkHash(
    ChunkPosX INT NOT NULL,
    ChunkPosZ INT NOT NULL,
    ChunkHash INT NOT NULL,
    LastModifiedUnixDateTime BIGINT NOT NULL,
    CreatedUnixDateTime BIGINT NOT NULL,
    PRIMARY KEY (ChunkPosX, ChunkPosZ)
);
CREATE TABLE BeaconBeam(
    BlockPosX INT NOT NULL,
    BlockPosY INT NOT NULL,
    BlockPosZ INT NOT NULL,
    ColorR INT NOT NULL,
    ColorG INT NOT NULL,
    ColorB INT NOT NULL,
    LastModifiedUnixDateTime BIGINT NOT NULL,
    CreatedUnixDateTime BIGINT NOT NULL,
    PRIMARY KEY (BlockPosX, BlockPosY, BlockPosZ)
);
";

/// Recorded as applied, so DH only runs scripts newer than these.
const APPLIED_SCRIPTS: [&str; 12] = [
    "0010-sqlite-createInitialDataTables.sql",
    "0020-sqlite-createFullDataSourceV2Tables.sql",
    "0030-sqlite-changeTableJournaling.sql",
    "0031-sqlite-useSqliteWalJournaling.sql",
    "0040-sqlite-removeRenderCache.sql",
    "0050-sqlite-addApplyToParentIndex.sql",
    "0060-sqlite-createChunkHashTable.sql",
    "0070-sqlite-createBeaconBeamTable.sql",
    "0080-sqlite-addApplyToChildrenColumn.sql",
    "0090-sqlite-addAdjacentFullDataColumns.sql",
    "0100-sqlite-deleteLowDetailDataForRegen.sql",
    "0110-sqlite-addApplyToParentIndex.sql",
];

const INSERT_ROW: &str = "
INSERT INTO FullData (
    DetailLevel, PosX, PosZ, MinY, DataChecksum,
    Data, ColumnGenerationStep, ColumnWorldCompressionMode, Mapping,
    NorthAdjData, SouthAdjData, EastAdjData, WestAdjData,
    DataFormatVersion, CompressionMode, ApplyToParent, ApplyToChildren, Regenerate,
    LastModifiedUnixDateTime, CreatedUnixDateTime)
VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0, 0, ?16, ?16)";

struct State {
    conn: Option<Connection>,
    error: Option<String>,
    sections: u64,
    bytes: u64,
}

/// Collects a world's LOD rows, shared across the region-save threads. Rows go into a
/// staging file that [`Self::finish`] moves into place, so DH never sees a partial one.
pub struct DhWriter {
    path: PathBuf,
    staging: PathBuf,
    created_ms: i64,
    state: Mutex<State>,
}

impl DhWriter {
    pub fn create(world_dir: &Path) -> Result<Self, BoxError> {
        let dir = world_dir.join("data");
        fs::create_dir_all(&dir)?;
        let path = dir.join(DB_NAME);
        let staging = dir.join(format!("{DB_NAME}.arnis-tmp"));
        // A regenerated world must not keep LODs of the terrain it replaced.
        remove_database(&path)?;
        remove_database(&staging)?;

        let conn = Connection::open(&staging)?;
        conn.execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;
             PRAGMA locking_mode = EXCLUSIVE;",
        )?;
        conn.execute_batch(SCHEMA)?;
        for script in APPLIED_SCRIPTS {
            conn.execute(
                "INSERT INTO Schema (ScriptName) VALUES (?1)",
                [format!("sqlScripts/{script}")],
            )?;
        }
        conn.execute_batch("BEGIN")?;

        let created_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        Ok(Self {
            path,
            staging,
            created_ms,
            state: Mutex::new(State {
                conn: Some(conn),
                error: None,
                sections: 0,
                bytes: 0,
            }),
        })
    }

    /// A builder for one region's levels 0..=3, against the world's build range.
    pub(crate) fn region_lod(&self) -> RegionLod<'_> {
        RegionLod::new(
            self,
            crate::world_editor::common::min_y(),
            crate::world_editor::common::world_max_y() + 1,
        )
    }

    /// Errors are latched for [`Self::finish`]: a failed LOD cache must not fail the world.
    pub(crate) fn put_section(
        &self,
        level: u8,
        x: i32,
        z: i32,
        row: &encode::Row,
        apply_to_parent: bool,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.error.is_some() {
            return;
        }
        let Some(conn) = state.conn.as_ref() else {
            return;
        };
        let result = conn.prepare_cached(INSERT_ROW).and_then(|mut stmt| {
            stmt.execute(params![
                level,
                x,
                z,
                row.checksum,
                row.data,
                row.gen_steps,
                row.compression_modes,
                row.mapping,
                row.north,
                row.south,
                row.east,
                row.west,
                encode::DATA_FORMAT_VERSION,
                encode::COMPRESSION_MODE,
                apply_to_parent,
                self.created_ms,
            ])
        });
        match result {
            Ok(_) => {
                state.sections += 1;
                state.bytes += row.len() as u64;
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }

    pub(crate) fn record_error(&self, error: String) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.error.get_or_insert(error);
    }

    /// Commits, switches to the WAL journal DH uses, and moves the file into place.
    pub fn finish(&self) -> Result<(u64, u64), BoxError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let conn = state
            .conn
            .take()
            .ok_or("the database was already finished")?;
        let sealed = match state.error.take() {
            Some(error) => {
                // Windows won't delete a file SQLite still holds open.
                drop(conn);
                Err(error.into())
            }
            None => seal(conn),
        };
        if let Err(e) = sealed {
            let _ = remove_database(&self.staging);
            return Err(e);
        }
        fs::rename(&self.staging, &self.path)?;
        Ok((state.sections, state.bytes))
    }
}

/// A run that fails before [`DhWriter::finish`] leaves no staging file behind.
impl Drop for DhWriter {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|p| p.into_inner());
        if let Some(conn) = state.conn.take() {
            drop(conn);
            let _ = remove_database(&self.staging);
        }
    }
}

fn seal(conn: Connection) -> Result<(), BoxError> {
    conn.execute_batch("COMMIT")?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(format!("could not switch the database to WAL (got {mode})").into());
    }
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

/// Removes a database and the journal files SQLite may have left beside it.
fn remove_database(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns_of(conn: &Connection, table: &str) -> Vec<String> {
        conn.prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn finish_leaves_a_database_dh_opens_as_current() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DhWriter::create(dir.path()).unwrap();
        let row = encode::Encoder::new()
            .encode(
                &vec![Vec::new(); encode::COLUMNS],
                &[0; encode::COLUMNS],
                &encode::Registry::new(),
            )
            .unwrap();
        writer.put_section(0, -3, 7, &row, false);
        assert_eq!(writer.finish().unwrap().0, 1);

        let data = dir.path().join("data");
        assert!(data.join(DB_NAME).is_file());
        for leftover in [
            format!("{DB_NAME}.arnis-tmp"),
            format!("{DB_NAME}-wal"),
            format!("{DB_NAME}-shm"),
        ] {
            assert!(!data.join(&leftover).exists(), "{leftover} left behind");
        }

        let conn = Connection::open(data.join(DB_NAME)).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");

        let scripts: Vec<String> = conn
            .prepare("SELECT ScriptName FROM Schema ORDER BY SchemaVersionId")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(scripts.len(), 12);
        assert_eq!(
            scripts[0],
            "sqlScripts/0010-sqlite-createInitialDataTables.sql"
        );
        assert_eq!(
            scripts[11],
            "sqlScripts/0110-sqlite-addApplyToParentIndex.sql"
        );

        // Every column DH's repository selects or upserts.
        let columns = columns_of(&conn, "FullData");
        for name in [
            "DetailLevel",
            "PosX",
            "PosZ",
            "MinY",
            "DataChecksum",
            "Data",
            "ColumnGenerationStep",
            "ColumnWorldCompressionMode",
            "Mapping",
            "NorthAdjData",
            "SouthAdjData",
            "EastAdjData",
            "WestAdjData",
            "DataFormatVersion",
            "CompressionMode",
            "ApplyToParent",
            "ApplyToChildren",
            "Regenerate",
            "LastModifiedUnixDateTime",
            "CreatedUnixDateTime",
        ] {
            assert!(columns.iter().any(|c| c == name), "FullData lacks {name}");
        }
        assert!(columns_of(&conn, "Legacy_FullData_V1").contains(&"MigrationFailed".to_string()));
        assert!(!columns_of(&conn, "ChunkHash").is_empty());
        assert!(!columns_of(&conn, "BeaconBeam").is_empty());

        let (level, x, z, version, mode, apply): (u8, i32, i32, u8, u8, bool) = conn
            .query_row(
                "SELECT DetailLevel, PosX, PosZ, DataFormatVersion, CompressionMode, ApplyToParent FROM FullData",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!((level, x, z, version, mode, apply), (0, -3, 7, 2, 4, false));
    }

    #[test]
    fn regenerating_replaces_the_old_database() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join(DB_NAME), b"stale").unwrap();
        fs::write(data.join(format!("{DB_NAME}-wal")), b"stale").unwrap();

        let writer = DhWriter::create(dir.path()).unwrap();
        assert!(!data.join(DB_NAME).exists());
        assert!(!data.join(format!("{DB_NAME}-wal")).exists());
        writer.finish().unwrap();
        let conn = Connection::open(data.join(DB_NAME)).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM FullData", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_latched_error_discards_the_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DhWriter::create(dir.path()).unwrap();
        writer.record_error("disk on fire".to_string());
        assert!(writer.finish().is_err());
        let data = dir.path().join("data");
        assert!(!data.join(DB_NAME).exists());
        assert!(!data.join(format!("{DB_NAME}.arnis-tmp")).exists());
    }

    #[test]
    fn an_unfinished_writer_cleans_up_when_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("data").join(format!("{DB_NAME}.arnis-tmp"));
        let writer = DhWriter::create(dir.path()).unwrap();
        assert!(staging.exists());
        drop(writer);
        assert!(!staging.exists());
    }
}
