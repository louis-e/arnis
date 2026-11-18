//! Chunks read back from a world's region files, in every form the game saves them, and light
//! baked into the ones saved without it.

use crate::live::region::{mcc_file, read_slots, region_file, slot_index, write_slots, Slot};
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

pub(crate) enum Saved {
    Absent,
    /// Present but unreadable.
    Bad,
    Chunk(Box<SavedChunk>),
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
    pub fn chunk(&self, x: i32, z: i32) -> Saved {
        let raw = match &self.entries[slot_index(x, z)] {
            None => return Saved::Absent,
            Some(Entry::Bad) => return Saved::Bad,
            Some(Entry::Inline(version, range)) => decompress(*version, &self.bytes[range.clone()]),
            Some(Entry::External(version)) => {
                std::fs::read(mcc_file(&self.dir, self.rx * 32 + x, self.rz * 32 + z))
                    .and_then(|data| decompress(*version, &data))
            }
        };
        match raw.ok().and_then(|nbt| parse(&nbt, false)) {
            Some(chunk) => Saved::Chunk(Box::new(chunk)),
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
    #[serde(default)]
    sections: Vec<SectionNbt>,
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

/// `keep_air` keeps the sections that hold only air.
fn parse(nbt: &[u8], keep_air: bool) -> Option<SavedChunk> {
    let ChunkNbt { sections } = fastnbt::from_bytes(nbt).ok()?;
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
    Some(SavedChunk {
        sections: out,
        biomes,
    })
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
}

/// Bakes light into every full chunk saved without it, as `--bake-lighting` writes it. Chunks
/// the game lit keep their light, which reaches across chunk borders.
pub(crate) fn bake_unlit(world_dir: &Path) -> Result<Baked, String> {
    use rayon::prelude::*;
    let (dir, regions) = overworld_regions(world_dir)?;
    let total = Mutex::new(Baked::default());
    regions.par_iter().for_each(|&(rx, rz)| {
        let path = region_file(&dir, rx, rz);
        let result = bake_region(&path);
        let mut t = total.lock().unwrap_or_else(|p| p.into_inner());
        match result {
            Ok((chunks, skipped)) => {
                t.chunks += chunks;
                t.regions += u64::from(chunks > 0);
                t.skipped += skipped;
            }
            Err(e) => eprintln!("Warning: light not baked into {}: {e}", path.display()),
        }
    });
    Ok(total.into_inner().unwrap_or_else(|p| p.into_inner()))
}

/// Returns the chunks baked and the unlit ones skipped; the file is rewritten only for the first.
fn bake_region(path: &Path) -> io::Result<(u64, u64)> {
    let mut slots = read_slots(path)?;
    let (mut baked, mut skipped) = (0, 0);
    for slot in &mut slots {
        let Slot::Inline { version, data } = slot else {
            continue;
        };
        let Ok(nbt) = decompress(*version, data) else {
            skipped += 1;
            continue;
        };
        if is_lit(&nbt) {
            continue;
        }
        match lit_chunk(&nbt) {
            Some(Some(lit)) => {
                let mut enc =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                enc.write_all(&lit)?;
                let packed = enc.finish()?;
                if packed.len() + 5 > MAX_INLINE_BYTES {
                    skipped += 1;
                    continue;
                }
                *slot = Slot::Inline {
                    version: 2,
                    data: packed,
                };
                baked += 1;
            }
            Some(None) => {}
            None => skipped += 1,
        }
    }
    if baked > 0 {
        write_slots(path, &slots)?;
    }
    Ok((baked, skipped))
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
    let chunk = parse(nbt, true)?;
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
