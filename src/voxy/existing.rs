//! The voxy LOD cache of a world already on disk, from its region files. No region is touched.

use super::{mark_live, LiveSections, VoxyWriter};
use crate::saved_chunks::{overworld_regions, Saved, SavedRegion};
use crate::world_editor::java::{chunk_section_span, compute_chunk_lighting, morton_16};
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Default)]
pub struct Summary {
    pub sections: u64,
    pub bytes: u64,
    pub skipped_regions: u64,
    pub skipped_chunks: u64,
}

/// Builds the overworld's voxy database from `region/*.mca`. Unreadable regions and chunks are
/// skipped and counted; the old database is replaced only once the new one is complete. A
/// region folder with no files gives an empty database, so removed land does not stay.
/// Gives up, keeping the old database, once `stop` says so.
pub fn from_existing(
    world_dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Summary, BoxError> {
    let writer = VoxyWriter::create_keeping_old(world_dir)?
        .ok_or("the world's seed could not be read from level.dat")?;
    let built = build(&writer, world_dir, stop).and_then(|mut summary| {
        (summary.sections, summary.bytes) = writer.finish()?;
        Ok(summary)
    });
    if built.is_err() {
        writer.abandon();
    }
    built
}

fn build(
    writer: &VoxyWriter,
    world_dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Summary, BoxError> {
    let (dir, regions) = overworld_regions(world_dir)?;
    let (read, bad_chunks) = (AtomicU64::new(0), AtomicU64::new(0));
    let bad_regions = Mutex::new(Vec::new());
    regions.par_iter().for_each(|&(rx, rz)| {
        if stop() {
            return;
        }
        match ingest_region(writer, &dir, rx, rz) {
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
    Ok(Summary {
        skipped_regions: bad_regions.len() as u64,
        skipped_chunks: bad_chunks.into_inner(),
        ..Default::default()
    })
}

/// Feeds one region to the writer; returns how many of its chunks could not be read, or
/// `None` for an empty file.
fn ingest_region(
    writer: &VoxyWriter,
    dir: &Path,
    rx: i32,
    rz: i32,
) -> std::io::Result<Option<u64>> {
    let Some(region) = SavedRegion::open(dir, rx, rz)? else {
        return Ok(None);
    };
    // The builder needs every section that can hold a block before the first chunk arrives;
    // a first pass reads only their heights, so a region is never held parsed whole.
    let (mut bad, mut present) = (0, [false; 1024]);
    let mut live = LiveSections::default();
    let (mut min_y, mut max_y) = (-4, None::<i32>);
    for (i, slot) in present.iter_mut().enumerate() {
        let (x, z) = ((i % 32) as i32, (i / 32) as i32);
        let ys = match region.block_ys(x, z) {
            Saved::Chunk(ys) => ys,
            Saved::Bad => {
                bad += 1;
                continue;
            }
            Saved::Absent => continue,
        };
        *slot = true;
        for y in ys.into_iter().map(i32::from) {
            mark_live(&mut live, rx * 32 + x, y, rz * 32 + z);
            min_y = min_y.min(y);
            max_y = Some(max_y.map_or(y, |m| m.max(y)));
        }
    }
    let Some(max_y) = max_y else {
        return Ok(Some(bad));
    };

    // One section past the top, as `region_content_span` gives the generation path.
    let mut lod = writer.region_lod(min_y, max_y + 1, live);
    for column in 0..256usize {
        let (sx, sz) = morton_16(column);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (x, z) = (sx * 2 + dx, sz * 2 + dz);
            if !present[(x + z * 32) as usize] {
                continue;
            }
            // The first pass read it; one that fails only now is left out.
            let Saved::Chunk(chunk) = region.chunk(x, z) else {
                continue;
            };
            let span = chunk_section_span(&chunk.sections);
            let lighting = compute_chunk_lighting(&chunk.sections, span);
            lod.ingest_chunk(
                rx * 32 + x,
                rz * 32 + z,
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
