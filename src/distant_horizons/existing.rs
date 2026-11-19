//! LODs for a world already on disk, from its region files; nothing is regenerated and no
//! region is touched.

use super::{BoxError, DhWriter, Summary};
use crate::saved_chunks::{overworld_regions, Saved, SavedRegion};
use crate::world_editor::java::{chunk_section_span, compute_chunk_lighting, morton_16};
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Builds the world's LOD database from the overworld's `region/*.mca`, cut to the build range
/// `y_range`. Unreadable regions and chunks are skipped and counted; the old database is
/// replaced only once the new one is complete. A region folder with no files gives an empty
/// database, so removed land does not stay.
/// Gives up, keeping the old database, once `stop` says so.
pub fn from_existing(
    world_dir: &Path,
    y_range: (i32, i32),
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Summary, BoxError> {
    let (region_dir, regions) = overworld_regions(world_dir)?;
    let writer = DhWriter::create_keeping_old(world_dir)?;
    let (read, bad_chunks) = (AtomicU64::new(0), AtomicU64::new(0));
    let bad_regions = Mutex::new(Vec::new());
    regions.par_iter().for_each(|&(rx, rz)| {
        if stop() {
            return;
        }
        match ingest_region(&writer, &region_dir, rx, rz, y_range) {
            Ok(None) => {}
            Ok(Some(bad)) => {
                read.fetch_add(1, Ordering::Relaxed);
                bad_chunks.fetch_add(bad, Ordering::Relaxed);
            }
            Err(e) => bad_regions
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("r.{rx}.{rz}.mca: {e}")),
        }
    });
    if stop() {
        return Err("stopped".into());
    }
    let mut bad_regions = bad_regions.into_inner().unwrap_or_else(|p| p.into_inner());
    bad_regions.sort();
    if read.into_inner() == 0 && !bad_regions.is_empty() {
        return Err(format!(
            "none of the {} region files could be read ({})",
            regions.len(),
            bad_regions[0]
        )
        .into());
    }
    for line in &bad_regions {
        eprintln!("Warning: skipped {line}");
    }
    let mut summary = writer.finish()?;
    summary.skipped_regions = bad_regions.len() as u64;
    summary.skipped_chunks = bad_chunks.into_inner();
    Ok(summary)
}

/// Feeds one region's chunks to the writer; returns how many could not be read, or `None`
/// for the empty files the game leaves behind.
fn ingest_region(
    writer: &DhWriter,
    dir: &Path,
    rx: i32,
    rz: i32,
    y_range: (i32, i32),
) -> Result<Option<u64>, BoxError> {
    let Some(region) = SavedRegion::open(dir, rx, rz)? else {
        return Ok(None);
    };
    let mut lod = writer.region_lod_within(y_range.0, y_range.1);
    let mut bad = 0;
    // Morton order, as the writer's builders expect (see `write_region_to_disk`).
    for column in 0..256usize {
        let (sx, sz) = morton_16(column);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (cx, cz) = (sx * 2 + dx, sz * 2 + dz);
            let chunk = match region.chunk(cx, cz) {
                Saved::Chunk(chunk) => chunk,
                Saved::Absent => continue,
                Saved::Bad => {
                    bad += 1;
                    continue;
                }
            };
            let span = chunk_section_span(&chunk.sections);
            let lighting = compute_chunk_lighting(&chunk.sections, span);
            lod.ingest_chunk(
                rx * 32 + cx,
                rz * 32 + cz,
                &chunk.sections,
                (i32::from(span.0), i32::from(span.1)),
                Some(lighting.as_slice()),
                &chunk.biomes,
            );
        }
        lod.end_column(column);
    }
    lod.finish();
    Ok(Some(bad))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastanvil::Region;
    use fastnbt::Value;

    /// A chunk NBT with stone in section 0 and the light-only section the game adds below.
    fn resaved_chunk() -> Vec<u8> {
        let compound = |pairs: Vec<(&str, Value)>| {
            Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        let stone = compound(vec![("Name", Value::String("minecraft:stone".into()))]);
        let blocks = compound(vec![
            ("Y", Value::Byte(0)),
            (
                "block_states",
                compound(vec![("palette", Value::List(vec![stone]))]),
            ),
        ]);
        let light = compound(vec![
            ("Y", Value::Byte(-5)),
            (
                "SkyLight",
                Value::ByteArray(fastnbt::ByteArray::new(vec![0; 2048])),
            ),
        ]);
        let chunk = compound(vec![
            ("Status", Value::String("minecraft:full".into())),
            ("sections", Value::List(vec![light, blocks])),
        ]);
        fastnbt::to_bytes(&chunk).unwrap()
    }

    /// The palette 26.x saves: bare names, no `Name` compounds.
    fn chunk_26x() -> Vec<u8> {
        let compound = |pairs: Vec<(&str, Value)>| {
            Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        let palette = ["minecraft:air", "minecraft:stone"].map(|n| Value::String(n.into()));
        let blocks = compound(vec![
            ("Y", Value::Byte(0)),
            (
                "block_states",
                compound(vec![
                    ("palette", Value::List(palette.to_vec())),
                    (
                        "data",
                        Value::LongArray(fastnbt::LongArray::new(vec![0x1111_1111; 256])),
                    ),
                ]),
            ),
        ]);
        let chunk = compound(vec![("sections", Value::List(vec![blocks]))]);
        fastnbt::to_bytes(&chunk).unwrap()
    }

    #[test]
    fn bad_regions_are_skipped_and_the_old_database_stays_until_the_new_is_done() {
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let world = tempfile::tempdir().unwrap();
        let regions = world.path().join("region");
        std::fs::create_dir_all(&regions).unwrap();
        let db = world.path().join("data").join(super::super::DB_NAME);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::write(&db, b"old").unwrap();

        // Only unreadable regions: the run fails and the old database is kept.
        std::fs::write(regions.join("r.1.0.mca"), b"garbage").unwrap();
        std::fs::write(regions.join("r.2.0.mca"), b"").unwrap();
        assert!(from_existing(world.path(), (-64, 319), &|| false).is_err());
        assert_eq!(std::fs::read(&db).unwrap(), b"old");

        // One good region, with one chunk that does not parse.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(regions.join("r.0.0.mca"))
            .unwrap();
        let mut region = Region::create(file).unwrap();
        region.write_chunk(3, 4, &resaved_chunk()).unwrap();
        region.write_chunk(4, 4, &chunk_26x()).unwrap();
        region.write_chunk(5, 4, b"not nbt").unwrap();
        drop(region);
        let summary = from_existing(world.path(), (-64, 319), &|| false).unwrap();
        assert!(summary.sections > 0);
        assert_eq!((summary.skipped_regions, summary.skipped_chunks), (1, 1));
        assert_ne!(std::fs::read(&db).unwrap(), b"old");
    }
}
