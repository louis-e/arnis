//! Chunks read back from a world's region files, in every form the game saves them, and light
//! baked into the ones saved without it.

use crate::live::region::{mcc_file, region_file, replace_atomically, slot_index};
use crate::world_editor::common::{Blockstates, PaletteItem, Section};
use fastnbt::{LongArray, Value};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const DEFAULT_BIOME: &str = "minecraft:plains";

/// A chunk's sections that hold a block, and its biome grid (`z * 4 + x`).
pub(crate) struct SavedChunk {
    pub sections: Vec<Section>,
    pub biomes: [&'static str; 16],
}

/// A slot of a region file. A chunk the game has not finished generating counts as absent.
pub(crate) enum Saved<T> {
    Absent,
    /// Present but unreadable.
    Bad,
    Chunk(T),
}

/// The overworld region folder of `world_dir` and the regions in it.
pub(crate) fn overworld_regions(world_dir: &Path) -> Result<(PathBuf, Vec<(i32, i32)>), String> {
    let dir = crate::world_utils::WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("region");
    let mut regions: Vec<(i32, i32)> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .filter_map(|e| crate::live::region::parse_region_name(&e.file_name().to_string_lossy()))
        .collect();
    regions.sort_unstable();
    Ok((dir, regions))
}

const SECTOR: usize = 4096;
const EXTERNAL: u8 = 0x80;

enum Entry {
    Inline(u8, std::ops::Range<usize>),
    /// In `c.X.Z.mcc`, with this compression.
    External(u8),
    Bad,
}

/// One region file, its chunks still compressed.
pub(crate) struct SavedRegion {
    dir: PathBuf,
    rx: i32,
    rz: i32,
    bytes: Vec<u8>,
    entries: Vec<Option<Entry>>,
}

impl SavedRegion {
    /// `None` for the empty files the game leaves behind.
    pub fn open(dir: &Path, rx: i32, rz: i32) -> io::Result<Option<Self>> {
        let bytes = std::fs::read(region_file(dir, rx, rz))?;
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes.len() < 2 * SECTOR {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shorter than a region header",
            ));
        }
        let entries = (0..1024)
            .map(|i| {
                let loc = u32::from_be_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
                if loc == 0 {
                    return None;
                }
                let start = (loc >> 8) as usize * SECTOR;
                if start < 2 * SECTOR || start + 5 > bytes.len() {
                    return Some(Entry::Bad);
                }
                let len = u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
                let version = bytes[start + 4];
                if version & EXTERNAL != 0 {
                    return Some(Entry::External(version & !EXTERNAL));
                }
                if len < 1 || start + 4 + len > bytes.len() {
                    return Some(Entry::Bad);
                }
                Some(Entry::Inline(version, start + 5..start + 4 + len))
            })
            .collect();
        Ok(Some(Self {
            dir: dir.to_path_buf(),
            rx,
            rz,
            bytes,
            entries,
        }))
    }

    /// The chunk at local `(x, z)`.
    pub fn chunk(&self, x: i32, z: i32) -> Saved<Box<SavedChunk>> {
        self.read(x, z, |nbt| parse(nbt, false).map(|c| c.map(Box::new)))
    }

    /// The `Y` of each section of the chunk at local `(x, z)` that holds a block, read without
    /// its block data.
    pub fn block_ys(&self, x: i32, z: i32) -> Saved<Vec<i8>> {
        self.read(x, z, block_ys)
    }

    fn read<T>(&self, x: i32, z: i32, parse: impl Fn(&[u8]) -> Option<Option<T>>) -> Saved<T> {
        let raw = match &self.entries[slot_index(x, z)] {
            None => return Saved::Absent,
            Some(Entry::Bad) => return Saved::Bad,
            Some(Entry::Inline(version, range)) => decompress(*version, &self.bytes[range.clone()]),
            Some(Entry::External(version)) => {
                std::fs::read(mcc_file(&self.dir, self.rx * 32 + x, self.rz * 32 + z))
                    .and_then(|data| decompress(*version, &data))
            }
        };
        match raw.ok().and_then(|nbt| parse(&nbt)) {
            Some(Some(chunk)) => Saved::Chunk(chunk),
            Some(None) => Saved::Absent,
            None => Saved::Bad,
        }
    }
}

fn decompress(version: u8, data: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match version {
        1 => {
            flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
        }
        2 => {
            flate2::read::ZlibDecoder::new(data).read_to_end(&mut out)?;
        }
        3 => out.extend_from_slice(data),
        4 => {
            let mut lz4 = fastanvil::Lz4DecoderWrapper::new(out);
            lz4.write_all(data)?;
            out = lz4
                .finish()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:?}")))?;
        }
        v => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown chunk compression {v}"),
            ))
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct ChunkNbt {
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(default)]
    sections: Vec<SectionNbt>,
}

/// Arnis and the game write `minecraft:full`; older saves wrote `full`.
fn is_full(status: Option<&str>) -> bool {
    status.is_none_or(|s| s == "minecraft:full" || s == "full")
}

#[derive(Deserialize)]
struct ChunkYs {
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(default)]
    sections: Vec<SectionYs>,
}

#[derive(Deserialize)]
struct SectionYs {
    #[serde(rename = "Y")]
    y: i8,
    block_states: Option<PaletteOnly>,
}

#[derive(Deserialize)]
struct PaletteOnly {
    #[serde(default)]
    palette: Vec<Value>,
}

/// `None` when it does not parse, `Some(None)` for a chunk not fully generated.
fn block_ys(nbt: &[u8]) -> Option<Option<Vec<i8>>> {
    let ChunkYs { status, sections } = fastnbt::from_bytes(nbt).ok()?;
    if !is_full(status.as_deref()) {
        return Some(None);
    }
    let mut ys = Vec::new();
    for s in sections {
        let Some(states) = s.block_states else {
            continue;
        };
        let names = states
            .palette
            .into_iter()
            .map(|e| palette_item(e).map(|p| p.name))
            .collect::<Option<Vec<_>>>()?;
        if !names.iter().all(|n| crate::voxy::is_air_name(n)) {
            ys.push(s.y);
        }
    }
    Some(Some(ys))
}

#[derive(Deserialize)]
struct SectionNbt {
    #[serde(rename = "Y")]
    y: i8,
    block_states: Option<Container>,
    biomes: Option<Container>,
}

#[derive(Deserialize)]
struct Container {
    #[serde(default)]
    palette: Vec<Value>,
    data: Option<LongArray>,
}

/// `keep_air` keeps the sections that hold only air. `None` when it does not parse,
/// `Some(None)` for a chunk not fully generated.
fn parse(nbt: &[u8], keep_air: bool) -> Option<Option<SavedChunk>> {
    let ChunkNbt { status, sections } = fastnbt::from_bytes(nbt).ok()?;
    if !is_full(status.as_deref()) {
        return Some(None);
    }
    let biomes = sections
        .iter()
        .find_map(|s| s.biomes.as_ref().and_then(biome_grid))
        .unwrap_or([DEFAULT_BIOME; 16]);
    let mut out = Vec::new();
    for s in sections {
        let Some(states) = s.block_states else {
            continue;
        };
        let palette = states
            .palette
            .into_iter()
            .map(palette_item)
            .collect::<Option<Vec<_>>>()?;
        if !keep_air && palette.iter().all(|p| crate::voxy::is_air_name(&p.name)) {
            continue;
        }
        out.push(Section {
            block_states: Blockstates {
                palette,
                data: states.data,
                other: Default::default(),
            },
            y: s.y,
            other: Default::default(),
        });
    }
    Some(Some(SavedChunk {
        sections: out,
        biomes,
    }))
}

/// Arnis writes `{Name, Properties}`; 26.x writes a bare name, `{"": name}` in a mixed list, or `{id, properties}`.
fn palette_item(entry: Value) -> Option<PaletteItem> {
    let mut map = match entry {
        Value::String(name) => {
            return Some(PaletteItem {
                name,
                properties: None,
            })
        }
        Value::Compound(map) => map,
        _ => return None,
    };
    let name = match map
        .remove("Name")
        .or_else(|| map.remove("id"))
        .or_else(|| map.remove(""))
    {
        Some(Value::String(name)) => name,
        _ => return None,
    };
    let properties = map
        .remove("Properties")
        .or_else(|| map.remove("properties"));
    Some(PaletteItem { name, properties })
}

fn biome_name(entry: &Value) -> Option<&'static str> {
    match entry {
        Value::String(s) => Some(intern(s)),
        Value::Compound(map) => match map.get("") {
            Some(Value::String(s)) => Some(intern(s)),
            _ => None,
        },
        _ => None,
    }
}

/// The bottom 4x4 layer of a section's biomes; Arnis repeats it up the column.
fn biome_grid(biomes: &Container) -> Option<[&'static str; 16]> {
    let palette: Vec<&'static str> = biomes.palette.iter().filter_map(biome_name).collect();
    let first = *palette.first()?;
    let mut names = [first; 16];
    if let (Some(data), true) = (&biomes.data, palette.len() > 1) {
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

#[derive(Debug, Default)]
pub(crate) struct Baked {
    pub chunks: u64,
    pub regions: u64,
    /// Unlit chunks left as they were: unreadable, or too large once lit.
    pub skipped: u64,
    /// Regions that could not be read or rewritten, with why.
    pub failed: Vec<String>,
}

/// Bakes light into every full chunk saved without it, as `--bake-lighting` writes it. Chunks
/// the game lit keep their light, which reaches across chunk borders. Once `stop` says so, the
/// regions not started yet are left as they are.
pub(crate) fn bake_unlit(
    world_dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
) -> Result<Baked, String> {
    use rayon::prelude::*;
    let (dir, regions) = overworld_regions(world_dir)?;
    let total = Mutex::new(Baked::default());
    regions.par_iter().for_each(|&(rx, rz)| {
        if stop() {
            return;
        }
        let result = bake_region(&dir, rx, rz);
        let mut t = total.lock().unwrap_or_else(|p| p.into_inner());
        match result {
            Ok((chunks, skipped)) => {
                t.chunks += chunks;
                t.regions += u64::from(chunks > 0);
                t.skipped += skipped;
            }
            Err(e) => t.failed.push(format!("r.{rx}.{rz}.mca: {e}")),
        }
    });
    let mut total = total.into_inner().unwrap_or_else(|p| p.into_inner());
    total.failed.sort();
    Ok(total)
}

/// Returns the chunks baked and the unlit ones skipped. The file is rewritten only when a chunk
/// was baked, with the other chunks and every timestamp as they were. A file with a damaged
/// entry is left alone.
fn bake_region(dir: &Path, rx: i32, rz: i32) -> io::Result<(u64, u64)> {
    let Some(region) = SavedRegion::open(dir, rx, rz)? else {
        return Ok((0, 0));
    };
    if region.entries.iter().any(|e| matches!(e, Some(Entry::Bad))) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "it has damaged chunk entries, so it is left as it was",
        ));
    }
    let (mut baked, mut skipped) = (0, 0);
    let mut lit: Vec<Option<Vec<u8>>> = vec![None; 1024];
    for (i, entry) in region.entries.iter().enumerate() {
        let Some(Entry::Inline(version, range)) = entry else {
            continue;
        };
        let Ok(nbt) = decompress(*version, &region.bytes[range.clone()]) else {
            skipped += 1;
            continue;
        };
        if is_lit(&nbt) {
            continue;
        }
        match lit_chunk(&nbt) {
            Some(Some(chunk)) => {
                let mut enc =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                enc.write_all(&chunk)?;
                let packed = enc.finish()?;
                if packed.len() + 5 > MAX_INLINE_BYTES {
                    skipped += 1;
                    continue;
                }
                lit[i] = Some(packed);
                baked += 1;
            }
            Some(None) => {}
            None => skipped += 1,
        }
    }
    if baked > 0 {
        replace_atomically(&region_file(dir, rx, rz), &region.with_chunks(&lit))?;
    }
    Ok((baked, skipped))
}

impl SavedRegion {
    /// The file again, with `replace[i]` (zlib) in slot `i` where given.
    fn with_chunks(&self, replace: &[Option<Vec<u8>>]) -> Vec<u8> {
        let mut header = self.bytes[..2 * SECTOR].to_vec();
        let mut body: Vec<u8> = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            let blob: Vec<u8> = match (entry, &replace[i]) {
                (None, _) | (Some(Entry::Bad), _) => {
                    header[i * 4..i * 4 + 4].fill(0);
                    continue;
                }
                (_, Some(zlib)) => {
                    [&(zlib.len() as u32 + 1).to_be_bytes()[..], &[2], zlib].concat()
                }
                (Some(Entry::Inline(version, range)), None) => {
                    let data = &self.bytes[range.clone()];
                    [
                        &(data.len() as u32 + 1).to_be_bytes()[..],
                        &[*version],
                        data,
                    ]
                    .concat()
                }
                (Some(Entry::External(version)), None) => {
                    [&1u32.to_be_bytes()[..], &[*version | EXTERNAL]].concat()
                }
            };
            let start = 2 + body.len() / SECTOR;
            let sectors = blob.len().div_ceil(SECTOR);
            body.extend_from_slice(&blob);
            body.resize((start - 2 + sectors) * SECTOR, 0);
            let loc = ((start as u32) << 8) | sectors as u32;
            header[i * 4..i * 4 + 4].copy_from_slice(&loc.to_be_bytes());
        }
        header.extend_from_slice(&body);
        header
    }
}

const MAX_INLINE_BYTES: usize = 255 * SECTOR;

fn is_lit(nbt: &[u8]) -> bool {
    const TAG: &[u8] = b"\x01\x00\x09isLightOn";
    nbt.windows(TAG.len() + 1)
        .any(|w| &w[..TAG.len()] == TAG && w[TAG.len()] != 0)
}

/// The chunk with its light baked in, `Some(None)` for one not fully generated.
fn lit_chunk(nbt: &[u8]) -> Option<Option<Vec<u8>>> {
    let Value::Compound(mut root) = fastnbt::from_bytes::<Value>(nbt).ok()? else {
        return None;
    };
    if !matches!(root.get("Status"), Some(Value::String(s)) if s == "minecraft:full") {
        return Some(None);
    }
    let chunk = parse(nbt, true)??;
    let span = crate::world_editor::java::chunk_section_span(&chunk.sections);
    let mut light = crate::world_editor::java::compute_chunk_lighting(&chunk.sections, span);
    let Some(Value::List(sections)) = root.get_mut("sections") else {
        return None;
    };
    for section in sections {
        let Value::Compound(section) = section else {
            continue;
        };
        let Some(&Value::Byte(y)) = section.get("Y") else {
            continue;
        };
        let Some((sky, block)) = usize::try_from(i32::from(y) - i32::from(span.0))
            .ok()
            .and_then(|i| light.get_mut(i))
            .filter(|(sky, _)| !sky.is_empty())
        else {
            continue;
        };
        section.insert(
            "SkyLight".to_string(),
            Value::ByteArray(fastnbt::ByteArray::new(std::mem::take(sky))),
        );
        section.insert(
            "BlockLight".to_string(),
            Value::ByteArray(fastnbt::ByteArray::new(std::mem::take(block))),
        );
    }
    root.insert("isLightOn".to_string(), Value::Byte(1));
    fastnbt::to_bytes(&Value::Compound(root)).ok().map(Some)
}

/// The LOD writers take `&'static str` biomes; the few dozen names live for the process.
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
    use crate::live::region::{write_slots, Slot, SLOTS};
    use std::collections::HashMap;

    fn compound(pairs: &[(&str, Value)]) -> Value {
        Value::Compound(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<HashMap<_, _>>(),
        )
    }

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn section(y: i8, palette: Vec<Value>) -> Value {
        compound(&[
            ("Y", Value::Byte(y)),
            (
                "block_states",
                compound(&[
                    ("palette", Value::List(palette)),
                    ("data", Value::LongArray(LongArray::new(vec![0; 256]))),
                ]),
            ),
            (
                "biomes",
                compound(&[("palette", Value::List(vec![s("minecraft:forest")]))]),
            ),
        ])
    }

    fn zlib(value: &Value) -> Vec<u8> {
        let nbt = fastnbt::to_bytes(value).unwrap();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&nbt).unwrap();
        enc.finish().unwrap()
    }

    /// The three palette forms 26.x saves, Arnis's own, and the air and light-only sections left out.
    fn chunk_26x() -> Value {
        let stairs = compound(&[
            ("id", s("minecraft:oak_stairs")),
            ("properties", compound(&[("facing", s("north"))])),
        ]);
        compound(&[(
            "sections",
            Value::List(vec![
                section(-4, vec![s("minecraft:stone"), s("minecraft:dirt")]),
                section(
                    0,
                    vec![compound(&[("", s("minecraft:air"))]), stairs.clone()],
                ),
                section(
                    1,
                    vec![compound(&[
                        ("Name", s("minecraft:glass")),
                        ("Properties", compound(&[])),
                    ])],
                ),
                section(2, vec![s("minecraft:air")]),
                compound(&[
                    ("Y", Value::Byte(-5)),
                    (
                        "SkyLight",
                        Value::ByteArray(fastnbt::ByteArray::new(vec![0; 2048])),
                    ),
                ]),
            ]),
        )])
    }

    #[test]
    fn reads_every_palette_form_inline_and_external() {
        let dir = tempfile::tempdir().unwrap();
        let mut slots = vec![Slot::Empty; SLOTS];
        slots[0] = Slot::Inline {
            version: 2,
            data: zlib(&chunk_26x()),
        };
        slots[1] = Slot::External { version: 2 };
        slots[2] = Slot::Inline {
            version: 2,
            data: b"not zlib".to_vec(),
        };
        write_slots(&dir.path().join("r.-1.0.mca"), &slots).unwrap();
        std::fs::write(dir.path().join("c.-31.0.mcc"), zlib(&chunk_26x())).unwrap();

        let region = SavedRegion::open(dir.path(), -1, 0).unwrap().unwrap();
        for x in [0, 1] {
            let Saved::Chunk(chunk) = region.chunk(x, 0) else {
                panic!("chunk {x} unread");
            };
            let ys: Vec<i8> = chunk.sections.iter().map(|s| s.y).collect();
            assert_eq!(ys, [-4, 0, 1], "the all-air section is left out");
            let names = |i: usize| -> Vec<&str> {
                chunk.sections[i]
                    .block_states
                    .palette
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect()
            };
            assert_eq!(names(0), ["minecraft:stone", "minecraft:dirt"]);
            assert_eq!(names(1), ["minecraft:air", "minecraft:oak_stairs"]);
            assert_eq!(names(2), ["minecraft:glass"]);
            let stairs = &chunk.sections[1].block_states.palette[1];
            assert_eq!(stairs.properties, Some(compound(&[("facing", s("north"))])));
            assert_eq!(chunk.biomes, ["minecraft:forest"; 16]);
        }
        assert!(matches!(region.chunk(2, 0), Saved::Bad));
        assert!(matches!(region.chunk(3, 0), Saved::Absent));

        // An empty file is no region; a short one is a broken one.
        std::fs::write(dir.path().join("r.0.0.mca"), b"").unwrap();
        assert!(SavedRegion::open(dir.path(), 0, 0).unwrap().is_none());
        std::fs::write(dir.path().join("r.0.0.mca"), b"garbage").unwrap();
        assert!(SavedRegion::open(dir.path(), 0, 0).is_err());
    }

    /// A chunk the game has not finished counts as absent; the first pass sees the same sections.
    #[test]
    fn unfinished_chunks_are_absent_and_heights_skip_air() {
        let dir = tempfile::tempdir().unwrap();
        let unfinished = compound(&[
            ("Status", s("minecraft:features")),
            (
                "sections",
                Value::List(vec![section(0, vec![s("minecraft:stone")])]),
            ),
        ]);
        let mut slots = vec![Slot::Empty; SLOTS];
        slots[0] = Slot::Inline {
            version: 2,
            data: zlib(&chunk_26x()),
        };
        slots[1] = Slot::Inline {
            version: 2,
            data: zlib(&unfinished),
        };
        write_slots(&dir.path().join("r.0.0.mca"), &slots).unwrap();
        let region = SavedRegion::open(dir.path(), 0, 0).unwrap().unwrap();
        assert!(matches!(region.chunk(1, 0), Saved::Absent));
        assert!(matches!(region.block_ys(1, 0), Saved::Absent));
        let Saved::Chunk(ys) = region.block_ys(0, 0) else {
            panic!("heights unread");
        };
        assert_eq!(ys, [-4, 0, 1]);
    }

    /// A bake rewrites only regions with an unlit chunk, keeps every other slot and timestamp,
    /// and leaves a region with a damaged entry alone.
    #[test]
    fn a_bake_keeps_the_rest_of_the_file_and_skips_damaged_ones() {
        let dir = tempfile::tempdir().unwrap();
        let regions = dir.path().join("region");
        std::fs::create_dir_all(&regions).unwrap();
        let unlit = compound(&[
            ("Status", s("minecraft:full")),
            ("isLightOn", Value::Byte(0)),
            (
                "sections",
                Value::List(vec![section(0, vec![s("minecraft:stone")])]),
            ),
        ]);
        let mut slots = vec![Slot::Empty; SLOTS];
        slots[0] = Slot::Inline {
            version: 2,
            data: zlib(&unlit),
        };
        slots[1] = Slot::External { version: 2 };
        let path = regions.join("r.0.0.mca");
        write_slots(&path, &slots).unwrap();
        let mut before = std::fs::read(&path).unwrap();
        before[4096..4100].copy_from_slice(&7u32.to_be_bytes());
        std::fs::write(&path, &before).unwrap();
        let damaged = regions.join("r.1.0.mca");
        let mut bad = before.clone();
        bad[8..12].copy_from_slice(&[0xFF, 0xFF, 0xFF, 1]);
        std::fs::write(&damaged, &bad).unwrap();

        let baked = bake_unlit(dir.path(), &|| false).unwrap();
        assert_eq!((baked.chunks, baked.regions), (1, 1));
        assert_eq!(baked.failed.len(), 1, "{:?}", baked.failed);
        assert_eq!(std::fs::read(&damaged).unwrap(), bad, "left alone");
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[4096..8192], &before[4096..8192], "timestamps kept");
        assert_eq!(&after[4..8], &[0, 0, 3, 1], "the external stub stays");
        let region = SavedRegion::open(&regions, 0, 0).unwrap().unwrap();
        let Some(Entry::Inline(version, range)) = &region.entries[0] else {
            panic!("chunk gone");
        };
        assert!(is_lit(
            &decompress(*version, &region.bytes[range.clone()]).unwrap()
        ));
        assert!(matches!(region.entries[1], Some(Entry::External(2))));
        assert_eq!(
            bake_unlit(dir.path(), &|| true).unwrap().chunks,
            0,
            "stopped"
        );
    }

    #[test]
    fn biome_grids_unpack_their_indices() {
        let biomes = Container {
            palette: vec![
                s("minecraft:plains"),
                compound(&[("", s("minecraft:beach"))]),
            ],
            // One bit per cell: cells 1 and 15 are beach.
            data: Some(LongArray::new(vec![(1 << 1) | (1 << 15)])),
        };
        let grid = biome_grid(&biomes).unwrap();
        assert_eq!(grid[1], "minecraft:beach");
        assert_eq!(grid[15], "minecraft:beach");
        assert_eq!(grid[0], "minecraft:plains");
    }
}
