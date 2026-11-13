//! Hytale saves: zstd BSON chunks in `HytaleIndexedStorage` regions, lit by the server on load.

use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::hytale_block_map::{to_hytale, HytaleCell, EMPTY};
use crate::progress::emit_gui_progress_update;
use crate::world_editor::common::{BlockStorage, ChunkToModify, SectionToModify, WorldToModify};
use base64::Engine as _;
use colored::Colorize;
use fnv::FnvHashMap;
use rayon::prelude::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Hytale's floor is Y 0, Arnis's -64: every block moves up by this.
const Y_OFFSET: i32 = 64;
/// Highest Arnis Y that fits under Hytale's 320-block column.
pub const HYTALE_MAX_Y: i32 = 320 - 1 - Y_OFFSET;

const SECTIONS: i32 = 10;
const SECTION_CELLS: usize = 32 * 32 * 32;
const COLUMNS: usize = 32 * 32;
const SEGMENT_SIZE: usize = 4096;
const BLOBS_PER_REGION: usize = 1024;
/// Block migrations the 0.5 server had run; any later ones still apply to these names.
const BLOCK_MIGRATION_VERSION: i32 = 11;
const BLOCK_SECTION_VERSION: i32 = 6;
const BLOCK_CHUNK_VERSION: i32 = 3;
/// The environment the void generator itself falls back to, so it always exists.
const ENVIRONMENT: &str = "Default";
const DEFAULT_TINT: u32 = 0xff5a_992b;

type SaveResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Creates the Hytale save folder and writes every chunk the run built.
#[allow(clippy::too_many_arguments)]
pub fn save_hytale_world(
    world: &WorldToModify,
    world_dir: &Path,
    xzbbox: &XZBBox,
    llbbox: &LLBBox,
    ground: Option<&Ground>,
    ground_origin: (i32, i32),
    level_name: Option<&str>,
    spawn_point: Option<(i32, i32)>,
    ground_level: i32,
    game_mode: crate::args::GameMode,
    world_time: i64,
) -> SaveResult<()> {
    println!("{} Saving Hytale world...", "[7/7]".bold());
    emit_gui_progress_update(90.0, "Saving Hytale world...");

    let world_folder = world_dir.join("universe").join("worlds").join("default");
    let chunks_dir = world_folder.join("chunks");
    fs::create_dir_all(&chunks_dir)?;

    let (x, z) = spawn_point.unwrap_or_else(|| xzbbox.center_spawn());
    let (sx, sy, sz) = find_spawn(world, x, z, ground_level);
    let name = level_name.unwrap_or("Arnis World");
    write_server_config(world_dir, game_mode)?;
    write_world_config(
        &world_folder,
        name,
        (sx, sy + Y_OFFSET, sz),
        game_mode,
        world_time,
    )?;

    let at = ground.and_then(|g| g.anchor()).unwrap_or(llbbox);
    let tints = Tints {
        ground,
        origin: ground_origin,
        center_lat: (at.min().lat() + at.max().lat()) * 0.5,
    };
    let regions = hytale_regions(world);
    let total = regions.len().max(1);
    let done = AtomicUsize::new(0);
    let written: usize = regions
        .par_iter()
        .map(|(&(rx, rz), chunks)| -> SaveResult<usize> {
            // Compressed as each chunk is built: a region's raw documents run to hundreds of MB.
            let blobs: Vec<Blob> = chunks
                .par_iter()
                .map(|&(cx, cz)| {
                    let slot = (cz.rem_euclid(32) * 32 + cx.rem_euclid(32)) as usize;
                    let raw = chunk_bson(world, cx, cz, &tints);
                    Ok((slot, raw.len(), zstd::bulk::compress(&raw, 3)?))
                })
                .collect::<std::io::Result<_>>()?;
            let path = chunks_dir.join(format!("{rx}.{rz}.region.bin"));
            write_region(&path, blobs)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            emit_gui_progress_update(
                90.0 + 9.0 * n as f64 / total as f64,
                "Saving Hytale world...",
            );
            Ok(chunks.len())
        })
        .collect::<SaveResult<Vec<usize>>>()?
        .into_iter()
        .sum();

    println!(
        "  Wrote {} Hytale chunks in {} region files",
        written.to_string().bold(),
        regions.len()
    );
    emit_gui_progress_update(99.0, "Hytale world saved.");
    println!(
        "{} Hytale world saved to: {}",
        "Done!".green().bold(),
        world_dir.display()
    );
    Ok(())
}

/// Hytale chunks (32x32 columns) holding anything, grouped by Hytale region file.
fn hytale_regions(world: &WorldToModify) -> BTreeMap<(i32, i32), Vec<(i32, i32)>> {
    let mut chunks: Vec<(i32, i32)> = world
        .regions
        .iter()
        .flat_map(|(&(rx, rz), region)| {
            region
                .chunks
                .iter()
                .map(move |((cx, cz), _)| (rx * 32 + cx, rz * 32 + cz))
        })
        .map(|(x, z)| (x.div_euclid(2), z.div_euclid(2)))
        .collect();
    chunks.sort_unstable();
    chunks.dedup();
    let mut regions: BTreeMap<(i32, i32), Vec<(i32, i32)>> = BTreeMap::new();
    for (x, z) in chunks {
        regions
            .entry((x.div_euclid(32), z.div_euclid(32)))
            .or_default()
            .push((x, z));
    }
    regions
}

fn arnis_chunk(world: &WorldToModify, x: i32, z: i32) -> Option<&ChunkToModify> {
    world
        .get_region(x.div_euclid(32), z.div_euclid(32))?
        .get_chunk(x.rem_euclid(32), z.rem_euclid(32))
}

/// Grass tint per column, from the biome Arnis would give the Java chunk.
struct Tints<'a> {
    ground: Option<&'a Ground>,
    origin: (i32, i32),
    center_lat: f64,
}

impl Tints<'_> {
    /// The 16 biome cells (4x4 blocks each) of one Arnis chunk, as ARGB tints.
    fn chunk(&self, x: i32, z: i32) -> [u32; 16] {
        let Some(ground) = self.ground else {
            return [DEFAULT_TINT; 16];
        };
        crate::biome::chunk_biome_names(x, z, Some(ground), self.center_lat, self.origin)
            .map(biome_tint)
    }
}

/// Minecraft grass colors, darkened to sit next to Hytale's own default green.
fn biome_tint(biome: &str) -> u32 {
    match biome.trim_start_matches("minecraft:") {
        "forest" | "birch_forest" | "flower_forest" => 0xff4f_8f2f,
        "dark_forest" => 0xff47_732a,
        "taiga" | "old_growth_pine_taiga" | "old_growth_spruce_taiga" => 0xff5c_8a4a,
        "jungle" | "sparse_jungle" | "bamboo_jungle" => 0xff3f_a02a,
        "swamp" | "mangrove_swamp" => 0xff5b_6b33,
        "savanna" | "savanna_plateau" => 0xffa3_9c45,
        "desert" => 0xffb5_a74e,
        "badlands" | "wooded_badlands" | "eroded_badlands" => 0xff9a_8240,
        "stony_peaks" | "stony_shore" => 0xff6e_8f55,
        "meadow" => 0xff63_a043,
        b if b.starts_with("snowy") || b.starts_with("frozen") || b.contains("peaks") => {
            0xff6c_9a7f
        }
        _ => DEFAULT_TINT,
    }
}

/// A section palette in insertion order; key 0 ("Empty", rotation 0) counts every unset cell.
struct Palette<K> {
    keys: Vec<K>,
    counts: Vec<u32>,
    ids: FnvHashMap<K, u16>,
    last: (K, u16),
}

impl<K: Copy + Eq + std::hash::Hash> Palette<K> {
    fn new(first: K) -> Self {
        let mut ids = FnvHashMap::default();
        ids.insert(first, 0);
        Self {
            keys: vec![first],
            counts: vec![0],
            ids,
            last: (first, 0),
        }
    }

    /// The key's id, counted once for the cell it goes into.
    fn add(&mut self, key: K) -> u16 {
        let id = if self.last.0 == key {
            self.last.1
        } else {
            let id = *self.ids.entry(key).or_insert_with(|| {
                self.keys.push(key);
                self.counts.push(0);
                (self.keys.len() - 1) as u16
            });
            self.last = (key, id);
            id
        };
        if id != 0 {
            self.counts[id as usize] += 1;
        }
        id
    }

    /// Writes palette type, entries and packed cells as `ISectionPalette` serializes them.
    fn write(&mut self, cells: &[u16], key: impl Fn(&mut Vec<u8>, &K), out: &mut Vec<u8>) {
        self.counts[0] = SECTION_CELLS as u32 - self.counts[1..].iter().sum::<u32>();
        let live: Vec<usize> = (0..self.keys.len())
            .filter(|&i| self.counts[i] > 0)
            .collect();
        if live.len() == 1 && live[0] == 0 {
            out.push(0);
            return;
        }
        let mut remap = vec![0u16; self.keys.len()];
        for (new, &old) in live.iter().enumerate() {
            remap[old] = new as u16;
        }
        let kind: u8 = match live.len() {
            0..=16 => 1,
            17..=256 => 2,
            _ => 3,
        };
        out.push(kind);
        out.extend_from_slice(&(live.len() as u16).to_be_bytes());
        for (new, &old) in live.iter().enumerate() {
            if kind == 3 {
                out.extend_from_slice(&(new as u16).to_be_bytes());
            } else {
                out.push(new as u8);
            }
            key(out, &self.keys[old]);
            // A full section's 32768 wraps to the short Java writes, 0x8000.
            out.extend_from_slice(&(self.counts[old] as u16).to_be_bytes());
        }
        match kind {
            1 => out.extend(
                cells
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&[a, b]| ((remap[a as usize] as u8) << 4) | remap[b as usize] as u8),
            ),
            2 => out.extend(cells.iter().map(|&c| remap[c as usize] as u8)),
            _ => {
                for &c in cells {
                    out.extend_from_slice(&remap[c as usize].to_be_bytes());
                }
            }
        }
    }

    /// Whether any cell holds a key other than the first.
    fn any(&self) -> bool {
        self.counts[1..].iter().any(|&c| c > 0)
    }
}

fn write_utf(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// A 32³ Hytale section from up to eight Arnis sections; cells allocate on the first set.
#[derive(Default)]
struct Section {
    cells: Option<Box<Cells>>,
}

struct Cells {
    blocks: Vec<u16>,
    rotations: Vec<u16>,
    fluids: Vec<u16>,
    block_palette: Palette<&'static str>,
    rotation_palette: Palette<u8>,
    fluid_palette: Palette<&'static str>,
}

impl Section {
    fn set(&mut self, index: usize, cell: HytaleCell) {
        let c = self.cells.get_or_insert_with(|| {
            Box::new(Cells {
                blocks: vec![0; SECTION_CELLS],
                rotations: vec![0; SECTION_CELLS],
                fluids: vec![0; SECTION_CELLS],
                block_palette: Palette::new(EMPTY),
                rotation_palette: Palette::new(0),
                fluid_palette: Palette::new(EMPTY),
            })
        });
        c.blocks[index] = c.block_palette.add(cell.block);
        c.rotations[index] = c.rotation_palette.add(cell.rotation);
        c.fluids[index] = c.fluid_palette.add(cell.fluid.unwrap_or(EMPTY));
    }

    fn has_blocks(&self) -> bool {
        self.cells.as_ref().is_some_and(|c| c.block_palette.any())
    }

    /// `BlockSection.serialize`: palette, no ticking, no fillers, rotations, no light.
    fn block_data(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SECTION_CELLS / 2 + 256);
        out.extend_from_slice(&BLOCK_MIGRATION_VERSION.to_be_bytes());
        let stale = u16::from(self.cells.is_some());
        match self.cells.as_deref_mut() {
            None => out.extend_from_slice(&[0, 0, 0]),
            Some(c) => {
                c.block_palette
                    .write(&c.blocks, |o, k| write_utf(o, k), &mut out);
                if out[4] != 0 {
                    // Ticking blocks: none; the server finds its own as they change.
                    out.extend_from_slice(&[0, 0, 0, 0]);
                }
                out.push(0);
                c.rotation_palette
                    .write(&c.rotations, |o, &k| o.push(k), &mut out);
            }
        }
        // No light stored and change counters ahead of it, so the server relights on load.
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&stale.to_be_bytes());
        out.extend_from_slice(&stale.to_be_bytes());
        out
    }

    /// `FluidSection.serialize`: type palette, then a nibble level per cell, low nibble first.
    fn fluid_data(&mut self) -> Vec<u8> {
        let Some(c) = self.cells.as_deref_mut().filter(|c| c.fluid_palette.any()) else {
            return vec![0, 0];
        };
        let mut out = Vec::with_capacity(SECTION_CELLS + 64);
        c.fluid_palette
            .write(&c.fluids, |o, k| write_utf(o, k), &mut out);
        out.push(1);
        let level = |f: u16| u8::from(f != 0);
        out.extend(
            c.fluids
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[a, b]| level(a) | (level(b) << 4)),
        );
        out
    }
}

/// Builds one Hytale chunk's BSON document.
fn chunk_bson(world: &WorldToModify, cx: i32, cz: i32, tints: &Tints) -> Vec<u8> {
    let arnis: [[Option<&ChunkToModify>; 2]; 2] =
        [0, 1].map(|dz| [0, 1].map(|dx| arnis_chunk(world, cx * 2 + dx, cz * 2 + dz)));
    let mut heights = [0i16; COLUMNS];
    let mut sections = Vec::with_capacity(SECTIONS as usize);
    for sy in 0..SECTIONS {
        let mut section = Section::default();
        for (dz, row) in arnis.iter().enumerate() {
            for (dx, chunk) in row.iter().enumerate() {
                let Some(chunk) = chunk else { continue };
                for dy in 0..2 {
                    let arnis_y = sy * 32 + dy * 16 - Y_OFFSET;
                    if let Some(s) = chunk.sections.get((arnis_y >> 4) as i8) {
                        fill(&mut section, s, dx * 16, dy as usize * 16, dz * 16);
                    }
                }
            }
        }
        if section.has_blocks() {
            let blocks = &section.cells.as_ref().expect("has blocks").blocks;
            for (i, &b) in blocks.iter().enumerate() {
                if b != 0 {
                    heights[i & (COLUMNS - 1)] = (sy * 32 + (i >> 10) as i32) as i16;
                }
            }
        }
        sections.push(section);
    }

    let mut tint = [DEFAULT_TINT; COLUMNS];
    for dz in 0..2 {
        for dx in 0..2 {
            let cells = tints.chunk(cx * 2 + dx as i32, cz * 2 + dz as i32);
            for z in 0..16 {
                for x in 0..16 {
                    tint[(dz * 16 + z) * 32 + dx * 16 + x] = cells[(z / 4) * 4 + x / 4];
                }
            }
        }
    }

    let section_docs: Vec<Doc> = sections
        .iter_mut()
        .map(|s| {
            Doc::new().doc(
                "Components",
                Doc::new()
                    .doc("ChunkSection", Doc::new())
                    .doc("BlockPhysics", Doc::new().binary("Data", &[0]))
                    .doc("Fluid", Doc::new().binary("Data", &s.fluid_data()))
                    .doc(
                        "Block",
                        Doc::new()
                            .int("Version", BLOCK_SECTION_VERSION)
                            .binary("Data", &s.block_data()),
                    ),
            )
        })
        .collect();

    Doc::new()
        .doc(
            "Components",
            Doc::new()
                .doc(
                    "BlockComponentChunk",
                    Doc::new().doc("BlockComponents", Doc::new()),
                )
                .doc("ChunkColumn", Doc::new().array("Sections", section_docs))
                .doc("WorldChunk", Doc::new())
                .doc(
                    "BlockHealthChunk",
                    Doc::new()
                        .binary("Data", &[2, 0, 0, 0, 0, 0, 0, 0, 0])
                        .string("LastRepairGameTime", "1970-01-01T00:00:00Z"),
                )
                .doc(
                    "ChunkSpawnedNPCData",
                    Doc::new().doc("EnvironmentSpawnCounts", Doc::new()),
                )
                .doc(
                    "EnvironmentChunk",
                    Doc::new().binary("Data", &environment_data()),
                )
                .doc(
                    "BlockChunk",
                    Doc::new()
                        .int("Version", BLOCK_CHUNK_VERSION)
                        .binary("Data", &block_chunk_data(&heights, &tint)),
                )
                .doc("EntityChunk", Doc::new().array("Entities", Vec::new())),
        )
        .finish()
}

/// Copies one Arnis section (YZX, 16 per side) into the Hytale section at an offset.
fn fill(section: &mut Section, from: &SectionToModify, ox: usize, oy: usize, oz: usize) {
    let air = crate::block_definitions::AIR;
    let at = |i: usize| ((oy + (i >> 8)) << 10) | ((oz + ((i >> 4) & 15)) << 5) | (ox + (i & 15));
    if let (BlockStorage::Uniform(block), true) = (&from.storage, from.properties.is_empty()) {
        let cell = to_hytale(*block, None);
        if *block != air && !(cell.is_empty() && cell.fluid.is_none()) {
            (0..4096).for_each(|i| section.set(at(i), cell));
        }
        return;
    }
    for i in 0..4096 {
        let block = from.get_block_at_index(i);
        let props = if from.properties.is_empty() {
            None
        } else {
            from.properties.get(&i)
        };
        if block == air && props.is_none() {
            continue;
        }
        let cell = to_hytale(block, props.map(|p| p.as_ref()));
        if cell.is_empty() && cell.fluid.is_none() {
            continue;
        }
        section.set(at(i), cell);
    }
}

/// The column nearest (x, z), within 40 blocks, with ground to stand on as Hytale sees it.
fn find_spawn(world: &WorldToModify, x: i32, z: i32, ground_level: i32) -> (i32, i32, i32) {
    for radius in 0i32..=40 {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() != radius && dz.abs() != radius {
                    continue;
                }
                if let Some(y) = spawn_y(world, x + dx, z + dz) {
                    return (x + dx, y, z + dz);
                }
            }
        }
    }
    // Water as far as the search goes: on its surface rather than in the ground below.
    let top = (-Y_OFFSET..=HYTALE_MAX_Y - 2)
        .rev()
        .find(|&y| world.get_block(x, y, z).is_some());
    (x, top.map_or(ground_level + 3, |y| y + 1), z)
}

/// Feet Y on the column's ground with two open cells above (under a tree is fine), unless
/// the ground is fluid or a cactus.
fn spawn_y(world: &WorldToModify, x: i32, z: i32) -> Option<i32> {
    let mut headroom = 2;
    for y in (-Y_OFFSET..=HYTALE_MAX_Y - 2).rev() {
        let cell = world
            .get_block(x, y, z)
            .map(|b| to_hytale(b, world.get_properties(x, y, z).as_deref()))
            .filter(|c| !c.is_empty() || c.fluid.is_some());
        let Some(cell) = cell else {
            headroom += 1;
            continue;
        };
        let name = cell.block;
        if name.starts_with("Plant_Leaves") || name.ends_with("_Trunk") {
            headroom = 0;
            continue;
        }
        let solid = cell.fluid.is_none() && !name.starts_with("Plant_Cactus");
        return (solid && headroom >= 2).then_some(y + 1);
    }
    None
}

/// `BlockChunk`: no pending physics, the heightmap and the tint map.
fn block_chunk_data(heights: &[i16; COLUMNS], tints: &[u32; COLUMNS]) -> Vec<u8> {
    let mut out = vec![0u8];
    let height_keys: Vec<i16> = column_palette(heights);
    write_column_palette(&mut out, heights, &height_keys, |o, &k| {
        o.extend_from_slice(&k.to_le_bytes())
    });
    let tint_keys: Vec<u32> = column_palette(tints);
    write_column_palette(&mut out, tints, &tint_keys, |o, &k| {
        o.extend_from_slice(&k.to_le_bytes())
    });
    out
}

fn column_palette<T: Copy + Ord>(values: &[T]) -> Vec<T> {
    let mut keys = values.to_vec();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// `ShortBytePalette`/`IntBytePalette`: LE keys, then a 10-bit LSB-first index per column.
fn write_column_palette<T: Copy + Ord>(
    out: &mut Vec<u8>,
    values: &[T],
    keys: &[T],
    key: impl Fn(&mut Vec<u8>, &T),
) {
    out.extend_from_slice(&(keys.len() as u16).to_le_bytes());
    for k in keys {
        key(out, k);
    }
    let mut bits = vec![0u8; COLUMNS * 10 / 8];
    for (i, v) in values.iter().enumerate() {
        let index = keys.binary_search(v).unwrap_or(0);
        for b in 0..10 {
            if index >> b & 1 == 1 {
                let bit = i * 10 + b;
                bits[bit / 8] |= 1 << (bit % 8);
            }
        }
    }
    out.extend_from_slice(&(bits.len() as i32).to_le_bytes());
    out.extend_from_slice(&bits);
}

/// `EnvironmentChunk`: one environment for every column, at every height.
fn environment_data() -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + COLUMNS * 8);
    out.extend_from_slice(&1i32.to_be_bytes());
    out.extend_from_slice(&0i32.to_be_bytes());
    write_utf(&mut out, ENVIRONMENT);
    for _ in 0..COLUMNS {
        out.extend_from_slice(&[0; 8]);
    }
    out
}

/// A chunk's slot in its region, its BSON length and the zstd-compressed BSON.
type Blob = (usize, usize, Vec<u8>);

/// Writes a region file: header, the 1024-slot index, then each blob in 4 KiB segments.
fn write_region(path: &Path, blobs: Vec<Blob>) -> SaveResult<()> {
    let segments = |data: &[u8]| (8 + data.len()).div_ceil(SEGMENT_SIZE);
    let mut index = vec![0u32; BLOBS_PER_REGION];
    let mut next_segment = 1;
    for (slot, _, data) in &blobs {
        index[*slot] = next_segment as u32;
        next_segment += segments(data);
    }
    let mut out = BufWriter::new(fs::File::create(path)?);
    out.write_all(b"HytaleIndexedStorage")?;
    for word in [1, BLOBS_PER_REGION as u32, SEGMENT_SIZE as u32]
        .into_iter()
        .chain(index)
    {
        out.write_all(&word.to_be_bytes())?;
    }
    let zeros = [0u8; SEGMENT_SIZE];
    for (_, raw_len, data) in &blobs {
        out.write_all(&(*raw_len as u32).to_be_bytes())?;
        out.write_all(&(data.len() as u32).to_be_bytes())?;
        out.write_all(data)?;
        out.write_all(&zeros[..segments(data) * SEGMENT_SIZE - 8 - data.len()])?;
    }
    out.flush()?;
    Ok(())
}

/// Just the BSON the chunk store reads: documents, arrays, binaries, ints and strings.
struct Doc(Vec<u8>);

impl Doc {
    fn new() -> Self {
        Doc(vec![0; 4])
    }

    fn key(&mut self, kind: u8, key: &str) {
        self.0.push(kind);
        self.0.extend_from_slice(key.as_bytes());
        self.0.push(0);
    }

    fn doc(mut self, key: &str, doc: Doc) -> Self {
        self.key(0x03, key);
        self.0.extend(doc.finish());
        self
    }

    fn array(mut self, key: &str, items: Vec<Doc>) -> Self {
        let mut array = Doc::new();
        for (i, item) in items.into_iter().enumerate() {
            array.key(0x03, &i.to_string());
            array.0.extend(item.finish());
        }
        self.key(0x04, key);
        self.0.extend(array.finish());
        self
    }

    fn binary(mut self, key: &str, data: &[u8]) -> Self {
        self.key(0x05, key);
        self.0.extend_from_slice(&(data.len() as i32).to_le_bytes());
        self.0.push(0);
        self.0.extend_from_slice(data);
        self
    }

    fn int(mut self, key: &str, value: i32) -> Self {
        self.key(0x10, key);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn string(mut self, key: &str, value: &str) -> Self {
        self.key(0x02, key);
        self.0
            .extend_from_slice(&(value.len() as i32 + 1).to_le_bytes());
        self.0.extend_from_slice(value.as_bytes());
        self.0.push(0);
        self
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.push(0);
        let len = self.0.len() as i32;
        self.0[..4].copy_from_slice(&len.to_le_bytes());
        self.0
    }
}

/// Hytale has Adventure and Creative; survival plays as Adventure.
fn hytale_game_mode(mode: crate::args::GameMode) -> &'static str {
    match mode {
        crate::args::GameMode::Survival => "Adventure",
        _ => "Creative",
    }
}

/// The server settings the singleplayer client starts the world with.
fn write_server_config(world_dir: &Path, game_mode: crate::args::GameMode) -> SaveResult<()> {
    let config = json!({
        "Version": 4,
        "ServerName": "Arnis",
        "MOTD": "",
        "Password": "",
        "MaxPlayers": 100,
        "MaxViewRadius": 32,
        "Defaults": { "World": "default", "GameMode": hytale_game_mode(game_mode) },
        "ConnectionTimeouts": {},
        "RateLimit": {},
        "Modules": {},
        "LogLevels": {},
        "Mods": {},
        "DisplayTmpTagsInStrings": false,
        "PlayerStorage": { "Type": "Hytale" },
        "Update": {},
        "WorldMap": {}
    });
    fs::write(
        world_dir.join("config.json"),
        serde_json::to_string_pretty(&config)?,
    )?;
    fs::write(
        world_dir.join("client_metadata.json"),
        serde_json::to_string_pretty(&json!({ "CreatedWithPatchline": "release" }))?,
    )?;
    Ok(())
}

/// The world's own config: void around the area, spawn, game mode and time of day.
fn write_world_config(
    world_folder: &Path,
    name: &str,
    spawn: (i32, i32, i32),
    game_mode: crate::args::GameMode,
    world_time: i64,
) -> SaveResult<()> {
    let uuid: [u8; 16] = rand::random();
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    // Minecraft's day starts at 06:00 and runs 1000 ticks an hour.
    let minutes = ((world_time.rem_euclid(24000) * 60 / 1000) + 6 * 60) % (24 * 60);
    let survival = game_mode == crate::args::GameMode::Survival;
    let config = json!({
        "Version": 4,
        "UUID": {
            "$binary": base64::engine::general_purpose::STANDARD.encode(uuid),
            "$type": "04"
        },
        "DisplayName": name,
        "Seed": seed,
        "SpawnProvider": {
            "Id": "Global",
            "SpawnPoint": {
                "X": spawn.0 as f64 + 0.5,
                "Y": spawn.1 as f64,
                "Z": spawn.2 as f64 + 0.5,
                "Pitch": 0.0,
                "Yaw": 0.0,
                "Roll": 0.0
            }
        },
        "WorldGen": { "Type": "Void", "Environment": ENVIRONMENT },
        "WorldMap": { "Type": "WorldGen" },
        "ChunkStorage": { "Type": "Hytale" },
        "ChunkConfig": {},
        "IsTicking": true,
        "IsBlockTicking": true,
        "IsPvpEnabled": false,
        "IsFallDamageEnabled": survival,
        "IsGameTimePaused": false,
        "GameTime": format!("0001-01-01T{:02}:{:02}:00Z", minutes / 60, minutes % 60),
        "RequiredPlugins": {},
        "GameMode": hytale_game_mode(game_mode),
        "IsSpawningNPC": false,
        "IsSpawnMarkersEnabled": true,
        "IsAllNPCFrozen": false,
        "GameplayConfig": "Default",
        "IsCompassUpdating": true,
        "IsSavingPlayers": true,
        "IsSavingChunks": true,
        "SaveNewChunks": true,
        "IsUnloadingChunks": true,
        "IsObjectiveMarkersEnabled": true,
        "DeleteOnUniverseStart": false,
        "DeleteOnRemove": false,
        "DisabledFluidTickers": [],
        "ResourceStorage": { "Type": "Hytale" },
        "Plugin": {}
    });
    fs::write(
        world_folder.join("config.json"),
        serde_json::to_string_pretty(&config)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bson_documents_carry_their_length() {
        let doc = Doc::new().int("Version", 6).finish();
        assert_eq!(doc.len(), 4 + 1 + 8 + 4 + 1);
        assert_eq!(
            i32::from_le_bytes(doc[..4].try_into().unwrap()),
            doc.len() as i32
        );
    }

    #[test]
    fn air_sections_are_empty_palettes() {
        let mut s = Section::default();
        let data = s.block_data();
        // Migration version, empty block palette, no fillers, no rotations, light, counters.
        assert_eq!(data.len(), 4 + 1 + 1 + 1 + 3 + 3 + 4);
        assert_eq!(data[4], 0);
        assert_eq!(s.fluid_data(), vec![0, 0]);
    }

    #[test]
    fn a_full_stone_section_matches_the_server_layout() {
        let mut s = Section::default();
        for i in 0..SECTION_CELLS {
            s.set(
                i,
                HytaleCell {
                    block: "Rock_Stone",
                    rotation: 0,
                    fluid: None,
                },
            );
        }
        let data = s.block_data();
        // As read from a 0.5 save: half-byte palette of one entry counted 0x8000.
        assert_eq!(&data[..7], &[0, 0, 0, 11, 1, 0, 1]);
        assert_eq!(&data[7..10], &[0, 0, 10]);
        assert_eq!(&data[10..20], b"Rock_Stone");
        assert_eq!(&data[20..22], &[0x80, 0]);
        assert_eq!(data.len(), 22 + 16384 + 4 + 1 + 1 + 3 + 3 + 4);
    }

    #[test]
    fn region_files_index_blobs_by_segment() {
        let dir = std::env::temp_dir().join(format!(
            "arnis-hytale-{}",
            crate::world_utils::unique_suffix()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0.0.region.bin");
        write_region(&path, vec![(5, 10, vec![1; 10]), (7, 9000, vec![2; 9000])]).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..20], b"HytaleIndexedStorage");
        let slot = |i: usize| u32::from_be_bytes(bytes[32 + i * 4..36 + i * 4].try_into().unwrap());
        assert_eq!((slot(5), slot(7), slot(0)), (1, 2, 0));
        let _ = fs::remove_dir_all(dir);
    }
}
