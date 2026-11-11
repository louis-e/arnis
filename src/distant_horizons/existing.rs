//! LODs for a world that is already on disk: reads its region files and feeds the same
//! writer a fresh `--dh-lod` run does. For worlds made without `--dh-lod`, or whose database
//! was lost; nothing is regenerated and no region is touched.

use super::{BoxError, DhWriter, Summary};
use crate::world_editor::common::Section;
use crate::world_editor::java::{chunk_section_span, compute_chunk_lighting, morton_16};
use fastanvil::Region;
use fastnbt::Value;
use rayon::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[derive(Deserialize)]
struct ChunkSections {
    sections: Vec<Section>,
}

/// Builds the world's LOD database from the overworld's `region/*.mca`.
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
    let writer = DhWriter::create(world_dir)?;
    regions.par_iter().for_each(|&(rx, rz)| {
        if let Err(e) = ingest_region(&writer, &region_dir, rx, rz) {
            writer.record_error(format!("r.{rx}.{rz}.mca: {e}"));
        }
    });
    writer.finish()
}

fn ingest_region(writer: &DhWriter, dir: &Path, rx: i32, rz: i32) -> Result<(), BoxError> {
    let mut region = Region::from_stream(File::open(dir.join(format!("r.{rx}.{rz}.mca")))?)?;
    let mut lod = writer.region_lod();
    // Morton order, as the writer's builders expect (see `write_region_to_disk`).
    for column in 0..256usize {
        let (sx, sz) = morton_16(column);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (cx, cz) = (sx * 2 + dx, sz * 2 + dz);
            let Ok(Some(raw)) = region.read_chunk(cx as usize, cz as usize) else {
                continue;
            };
            let Ok(ChunkSections { sections }) = fastnbt::from_bytes(&raw) else {
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
    Ok(())
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
