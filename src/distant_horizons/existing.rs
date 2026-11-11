//! LODs for a world already on disk, from its region files; nothing is regenerated and no
//! region is touched.

use super::{BoxError, DhWriter, Summary};
use crate::world_editor::common::{Blockstates, Section};
use crate::world_editor::java::{chunk_section_span, compute_chunk_lighting, morton_16};
use fastanvil::Region;
use fastnbt::Value;
use fnv::FnvHashMap;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Deserialize)]
struct ChunkSections {
    sections: Vec<SavedSection>,
}

/// A section as the game saves it: the light-only ones above and below the blocks have no
/// `block_states`.
#[derive(Deserialize)]
struct SavedSection {
    block_states: Option<Blockstates>,
    #[serde(rename = "Y")]
    y: i8,
    #[serde(flatten)]
    other: FnvHashMap<String, Value>,
}

/// The block sections of a chunk's NBT, or `None` if it does not parse.
fn chunk_sections(raw: &[u8]) -> Option<Vec<Section>> {
    let ChunkSections { sections } = fastnbt::from_bytes(raw).ok()?;
    Some(
        sections
            .into_iter()
            .filter_map(|s| {
                Some(Section {
                    block_states: s.block_states?,
                    y: s.y,
                    other: s.other,
                })
            })
            .collect(),
    )
}

/// Builds the world's LOD database from the overworld's `region/*.mca`. Unreadable regions
/// and chunks are skipped and counted; the old database is replaced only once the new one
/// is complete.
pub fn from_existing(world_dir: &Path) -> Result<Summary, BoxError> {
    let region_dir = crate::world_utils::WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("region");
    let regions: Vec<(i32, i32)> = std::fs::read_dir(&region_dir)
        .map_err(|e| format!("{}: {e}", region_dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let mut parts = name.strip_suffix(".mca")?.split('.');
            let (Some("r"), Some(x), Some(z), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return None;
            };
            Some((x.parse().ok()?, z.parse().ok()?))
        })
        .collect();
    if regions.is_empty() {
        return Err(format!("{} has no .mca region files", region_dir.display()).into());
    }
    let writer = DhWriter::create_keeping_old(world_dir)?;
    let (read, bad_chunks) = (AtomicU64::new(0), AtomicU64::new(0));
    let bad_regions = Mutex::new(Vec::new());
    regions.par_iter().for_each(
        |&(rx, rz)| match ingest_region(&writer, &region_dir, rx, rz) {
            Ok(None) => {}
            Ok(Some(bad)) => {
                read.fetch_add(1, Ordering::Relaxed);
                bad_chunks.fetch_add(bad, Ordering::Relaxed);
            }
            Err(e) => bad_regions
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("r.{rx}.{rz}.mca: {e}")),
        },
    );
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
fn ingest_region(writer: &DhWriter, dir: &Path, rx: i32, rz: i32) -> Result<Option<u64>, BoxError> {
    let file = File::open(dir.join(format!("r.{rx}.{rz}.mca")))?;
    if file.metadata()?.len() == 0 {
        return Ok(None);
    }
    let mut region = Region::from_stream(file)?;
    let mut lod = writer.region_lod();
    let mut bad = 0;
    // Morton order, as the writer's builders expect (see `write_region_to_disk`).
    for column in 0..256usize {
        let (sx, sz) = morton_16(column);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (cx, cz) = (sx * 2 + dx, sz * 2 + dz);
            let raw = match region.read_chunk(cx as usize, cz as usize) {
                Ok(Some(raw)) => raw,
                Ok(None) => continue,
                Err(_) => {
                    bad += 1;
                    continue;
                }
            };
            let Some(sections) = chunk_sections(&raw) else {
                bad += 1;
                continue;
            };
            if sections.is_empty() {
                continue;
            }
            let span = chunk_section_span(&sections);
            let lighting = compute_chunk_lighting(&sections, span);
            let biomes = sections
                .iter()
                .find_map(biome_names)
                .unwrap_or(["minecraft:plains"; 16]);
            lod.ingest_chunk(
                rx * 32 + cx,
                rz * 32 + cz,
                &sections,
                (i32::from(span.0), i32::from(span.1)),
                Some(&lighting),
                &biomes,
            );
        }
        lod.end_column(column);
    }
    lod.finish();
    Ok(Some(bad))
}

/// The 4x4 biome grid a section stores (`zi * 4 + xi`; Arnis repeats it up the column).
fn biome_names(section: &Section) -> Option<[&'static str; 16]> {
    let Some(Value::Compound(biomes)) = section.other.get("biomes") else {
        return None;
    };
    let Some(Value::List(palette)) = biomes.get("palette") else {
        return None;
    };
    let palette: Vec<&'static str> = palette
        .iter()
        .filter_map(|v| match v {
            Value::String(s) => Some(intern(s)),
            _ => None,
        })
        .collect();
    let first = *palette.first()?;
    let mut names = [first; 16];
    if let (Some(Value::LongArray(data)), true) = (biomes.get("data"), palette.len() > 1) {
        let bits = (palette.len() - 1).ilog2() + 1;
        let per_long = (64 / bits) as usize;
        for (cell, name) in names.iter_mut().enumerate() {
            let long = data.get(cell / per_long).copied().unwrap_or(0) as u64;
            let index = (long >> ((cell % per_long) as u32 * bits)) & ((1 << bits) - 1);
            *name = palette.get(index as usize).copied().unwrap_or(first);
        }
    }
    Some(names)
}

/// Biome names are `&'static str` in the writer; the few dozen distinct ones live for the run.
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(&known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(name.to_string(), leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_chunk_the_game_saved_again_keeps_its_block_sections() {
        let sections = chunk_sections(&resaved_chunk()).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].y, 0);
        assert_eq!(sections[0].block_states.palette[0].name, "minecraft:stone");
        assert!(chunk_sections(b"not nbt").is_none());
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
        assert!(from_existing(world.path()).is_err());
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
        region.write_chunk(5, 4, b"not nbt").unwrap();
        drop(region);
        let summary = from_existing(world.path()).unwrap();
        assert!(summary.sections > 0);
        assert_eq!((summary.skipped_regions, summary.skipped_chunks), (1, 1));
        assert_ne!(std::fs::read(&db).unwrap(), b"old");
    }
}
