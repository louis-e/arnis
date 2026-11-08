//! Region file surgery for live delivery. The server keeps an open region's header in
//! memory, so gaps are filled through stubs: slots it reads from `c.X.Z.mcc` on every load.

use fastnbt::Value;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const SLOTS: usize = 1024;
const SECTOR: usize = 4096;
const EXTERNAL: u8 = 0x80;
const ZLIB: u8 = 2;
/// The header packs a chunk's sector count into one byte.
const MAX_INLINE_SECTORS: usize = 255;

#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Empty,
    Inline {
        version: u8,
        data: Vec<u8>,
    },
    /// Data lives in `c.X.Z.mcc`; `version` is the compression without the flag.
    External {
        version: u8,
    },
}

/// One bit per chunk slot of a region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bits(pub [u64; 16]);

impl Bits {
    pub fn all() -> Self {
        Bits([u64::MAX; 16])
    }
    pub fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    pub fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    pub fn count(&self) -> u32 {
        self.0.iter().map(|w| w.count_ones()).sum()
    }
    pub fn is_full(&self) -> bool {
        self.0.iter().all(|w| *w == u64::MAX)
    }
    pub fn union(&self, other: &Bits) -> Bits {
        let mut out = *self;
        for (a, b) in out.0.iter_mut().zip(other.0.iter()) {
            *a |= *b;
        }
        out
    }
}

pub fn slot_index(cx: i32, cz: i32) -> usize {
    ((cx & 31) + (cz & 31) * 32) as usize
}

pub fn region_file(region_dir: &Path, rx: i32, rz: i32) -> PathBuf {
    region_dir.join(format!("r.{rx}.{rz}.mca"))
}

pub fn mcc_file(region_dir: &Path, cx: i32, cz: i32) -> PathBuf {
    region_dir.join(format!("c.{cx}.{cz}.mcc"))
}

/// Slots that hold anything, read from the header alone. `None` without a file.
pub fn occupied(path: &Path) -> io::Result<Option<Bits>> {
    let mut header = [0u8; SECTOR];
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut read = 0;
    while read < SECTOR {
        let n = file.read(&mut header[read..])?;
        if n == 0 {
            break;
        }
        read += n;
    }
    let mut bits = Bits::default();
    for i in 0..SLOTS {
        if read >= i * 4 + 4 && header[i * 4..i * 4 + 4] != [0; 4] {
            bits.set(i);
        }
    }
    Ok(Some(bits))
}

/// Slots still pointing at an external chunk, from the header and each chunk's first bytes.
pub fn external_slots(path: &Path) -> io::Result<Bits> {
    use std::io::{Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let mut header = [0u8; SECTOR];
    file.read_exact(&mut header)?;
    let mut bits = Bits::default();
    for i in 0..SLOTS {
        let entry = u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
        if entry >> 8 < 2 {
            continue;
        }
        let mut head = [0u8; 5];
        file.seek(SeekFrom::Start(u64::from(entry >> 8) * SECTOR as u64))?;
        if file.read_exact(&mut head).is_ok() && head[4] & EXTERNAL != 0 {
            bits.set(i);
        }
    }
    Ok(bits)
}

/// Every slot of a region file, payloads still compressed.
pub fn read_slots(path: &Path) -> io::Result<Vec<Slot>> {
    let bytes = fs::read(path)?;
    let mut slots = vec![Slot::Empty; SLOTS];
    if bytes.len() < 2 * SECTOR {
        return Ok(slots);
    }
    for (i, slot) in slots.iter_mut().enumerate() {
        let entry = u32::from_be_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        if entry == 0 {
            continue;
        }
        let start = (entry >> 8) as usize * SECTOR;
        if start < 2 * SECTOR || start + 5 > bytes.len() {
            continue;
        }
        let len = u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
        let version = bytes[start + 4];
        if version & EXTERNAL != 0 {
            *slot = Slot::External {
                version: version & !EXTERNAL,
            };
            continue;
        }
        if len < 1 || start + 4 + len > bytes.len() {
            continue;
        }
        *slot = Slot::Inline {
            version,
            data: bytes[start + 5..start + 4 + len].to_vec(),
        };
    }
    Ok(slots)
}

/// Writes a region file in one go and swaps it in. Only for files the server has not opened.
pub fn write_slots(path: &Path, slots: &[Slot]) -> io::Result<()> {
    let mut offsets = vec![0u32; SLOTS];
    let mut body: Vec<u8> = Vec::new();
    let mut sector = 2usize;
    for (i, slot) in slots.iter().enumerate() {
        let (version, data): (u8, &[u8]) = match slot {
            Slot::Empty => continue,
            Slot::Inline { version, data } => (*version, data),
            Slot::External { version } => (*version | EXTERNAL, &[]),
        };
        let mut blob = Vec::with_capacity(data.len() + 5);
        blob.extend_from_slice(&((data.len() + 1) as u32).to_be_bytes());
        blob.push(version);
        blob.extend_from_slice(data);
        let sectors = blob.len().div_ceil(SECTOR);
        if sectors > MAX_INLINE_SECTORS {
            return Err(io::Error::other(format!(
                "chunk slot {i} is too large to inline"
            )));
        }
        blob.resize(sectors * SECTOR, 0);
        offsets[i] = ((sector as u32) << 8) | sectors as u32;
        sector += sectors;
        body.extend_from_slice(&blob);
    }
    let now = unix_now() as u32;
    let mut out = Vec::with_capacity(2 * SECTOR + body.len());
    for o in &offsets {
        out.extend_from_slice(&o.to_be_bytes());
    }
    for o in &offsets {
        out.extend_from_slice(&(if *o == 0 { 0 } else { now }).to_be_bytes());
    }
    out.extend_from_slice(&body);
    replace_atomically(path, &out)
}

/// Turns every empty slot into a stub, so its chunk can arrive later as a `.mcc`.
/// Returns the slots that became stubs.
pub fn add_stubs(path: &Path) -> io::Result<Bits> {
    let mut slots = read_slots(path)?;
    let mut stubbed = Bits::default();
    for (i, slot) in slots.iter_mut().enumerate() {
        if *slot == Slot::Empty {
            *slot = Slot::External { version: ZLIB };
            stubbed.set(i);
        }
    }
    if stubbed.count() > 0 {
        write_slots(path, &slots)?;
    }
    Ok(stubbed)
}

/// Delivers one chunk behind a stub. Atomic, so the server never reads half a file.
pub fn write_mcc(region_dir: &Path, cx: i32, cz: i32, zlib_data: &[u8]) -> io::Result<()> {
    replace_atomically(&mcc_file(region_dir, cx, cz), zlib_data)
}

/// Links `src` in as `dst` unless `dst` exists. Returns false when it does.
pub fn link_no_replace(src: &Path, dst: &Path) -> io::Result<bool> {
    match fs::hard_link(src, dst) {
        Ok(()) => {
            let _ = fs::remove_file(src);
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        // No hard links on this volume (FAT and the like): the rename leaves a tiny window.
        Err(_) if !dst.exists() => fs::rename(src, dst).map(|_| true),
        Err(_) => Ok(false),
    }
}

/// After a session: stubs with a delivered `.mcc` become ordinary chunks, the rest are
/// cleared. Returns how many slots changed.
pub fn normalize(path: &Path, region_dir: &Path, rx: i32, rz: i32) -> io::Result<usize> {
    let mut slots = read_slots(path)?;
    let mut changed = 0;
    for (i, slot) in slots.iter_mut().enumerate() {
        let Slot::External { version } = *slot else {
            continue;
        };
        let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
        let mcc = mcc_file(region_dir, cx, cz);
        match fs::read(&mcc) {
            // A chunk too large to inline stays external, as the game wrote it.
            Ok(data) if (data.len() + 5).div_ceil(SECTOR) <= MAX_INLINE_SECTORS => {
                *slot = Slot::Inline { version, data };
                changed += 1;
            }
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                *slot = Slot::Empty;
                changed += 1;
            }
            Err(e) => return Err(e),
        }
    }
    if changed > 0 {
        write_slots(path, &slots)?;
        for (i, slot) in slots.iter().enumerate() {
            if matches!(slot, Slot::Inline { .. }) {
                let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
                let _ = fs::remove_file(mcc_file(region_dir, cx, cz));
            }
        }
    }
    Ok(changed)
}

/// Fills slots of `live` that are empty, stubs, or hold a chunk the server made of nothing
/// with the generated chunks in `staged`. Returns how many were filled.
pub fn fill_from(
    live: &Path,
    staged: &Path,
    region_dir: &Path,
    rx: i32,
    rz: i32,
) -> io::Result<usize> {
    let staged_slots = read_slots(staged)?;
    if !live.exists() {
        let filled = staged_slots.iter().filter(|s| **s != Slot::Empty).count();
        fs::copy(staged, live)?;
        return Ok(filled);
    }
    let mut slots = read_slots(live)?;
    let mut filled = 0;
    for (i, (slot, new)) in slots.iter_mut().zip(staged_slots).enumerate() {
        if new == Slot::Empty {
            continue;
        }
        let replace = match slot {
            Slot::Empty => true,
            Slot::Inline { version, data } => is_void_chunk(*version, data),
            // A stub left without its chunk; one with a `.mcc` is a chunk too large to inline.
            Slot::External { .. } => {
                let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
                !mcc_file(region_dir, cx, cz).exists()
            }
        };
        if replace {
            *slot = new;
            filled += 1;
        }
    }
    if filled > 0 {
        write_slots(live, &slots)?;
    }
    Ok(filled)
}

/// Empties the slots holding chunks the server made of nothing, so they are generated again.
pub fn clear_void(path: &Path) -> io::Result<usize> {
    let mut slots = read_slots(path)?;
    let mut cleared = 0;
    for slot in slots.iter_mut() {
        if let Slot::Inline { version, data } = slot {
            if is_void_chunk(*version, data) {
                *slot = Slot::Empty;
                cleared += 1;
            }
        }
    }
    if cleared > 0 {
        write_slots(path, &slots)?;
    }
    Ok(cleared)
}

/// Whether a chunk holds nothing but air, or never finished generating: what the server
/// writes where it reached a chunk before Arnis did.
pub fn is_void_chunk(version: u8, data: &[u8]) -> bool {
    let raw = match version {
        1 => {
            let mut out = Vec::new();
            if flate2::read::GzDecoder::new(data)
                .read_to_end(&mut out)
                .is_err()
            {
                return false;
            }
            out
        }
        2 => {
            let mut out = Vec::new();
            if flate2::read::ZlibDecoder::new(data)
                .read_to_end(&mut out)
                .is_err()
            {
                return false;
            }
            out
        }
        3 => data.to_vec(),
        _ => return false,
    };
    let Ok(Value::Compound(root)) = fastnbt::from_bytes::<Value>(&raw) else {
        return false;
    };
    if let Some(Value::String(status)) = root.get("Status") {
        if status.trim_start_matches("minecraft:") != "full" {
            return true;
        }
    }
    for key in ["block_entities", "entities"] {
        if matches!(root.get(key), Some(Value::List(l)) if !l.is_empty()) {
            return false;
        }
    }
    let Some(Value::List(sections)) = root.get("sections") else {
        return true;
    };
    for section in sections {
        let Value::Compound(section) = section else {
            continue;
        };
        let Some(Value::Compound(states)) = section.get("block_states") else {
            continue;
        };
        let Some(Value::List(palette)) = states.get("palette") else {
            continue;
        };
        for entry in palette {
            let name = match entry {
                Value::String(s) => s.as_str(),
                Value::Compound(c) => match c.get("Name") {
                    Some(Value::String(s)) => s.as_str(),
                    _ => continue,
                },
                _ => continue,
            };
            if !matches!(
                name,
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            ) {
                return false;
            }
        }
    }
    true
}

/// Region coordinates from `r.X.Z.mca`.
pub fn parse_region_name(name: &str) -> Option<(i32, i32)> {
    let mut parts = name.strip_prefix("r.")?.strip_suffix(".mca")?.split('.');
    let rx = parts.next()?.parse().ok()?;
    let rz = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((rx, rz))
}

pub fn replace_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("region");
    let tmp = path.with_file_name(format!(".{name}.arnis-tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn zlib_chunk(status: &str, block: &str) -> Vec<u8> {
        let mut palette_entry = HashMap::new();
        palette_entry.insert("Name".to_string(), Value::String(block.to_string()));
        let mut states = HashMap::new();
        states.insert(
            "palette".to_string(),
            Value::List(vec![Value::Compound(palette_entry)]),
        );
        let mut section = HashMap::new();
        section.insert("block_states".to_string(), Value::Compound(states));
        let mut root = HashMap::new();
        root.insert("Status".to_string(), Value::String(status.to_string()));
        root.insert(
            "sections".to_string(),
            Value::List(vec![Value::Compound(section)]),
        );
        let raw = fastnbt::to_bytes(&Value::Compound(root)).unwrap();
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn slots_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.0.0.mca");
        let mut slots = vec![Slot::Empty; SLOTS];
        slots[0] = Slot::Inline {
            version: 2,
            data: zlib_chunk("minecraft:full", "minecraft:stone"),
        };
        slots[5] = Slot::External { version: 2 };
        write_slots(&path, &slots).unwrap();
        assert_eq!(read_slots(&path).unwrap(), slots);
        let occ = occupied(&path).unwrap().unwrap();
        assert!(occ.get(0) && occ.get(5) && !occ.get(1));
        assert_eq!(occ.count(), 2);
    }

    #[test]
    fn stubs_fill_the_gaps_and_normalize_inlines_what_arrived() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.1.2.mca");
        let stone = zlib_chunk("minecraft:full", "minecraft:stone");
        let mut slots = vec![Slot::Empty; SLOTS];
        slots[0] = Slot::Inline {
            version: 2,
            data: stone.clone(),
        };
        write_slots(&path, &slots).unwrap();

        let stubbed = add_stubs(&path).unwrap();
        assert_eq!(stubbed.count(), 1023);
        assert!(!stubbed.get(0));
        let external = external_slots(&path).unwrap();
        assert_eq!(external.count(), 1023);
        assert!(!external.get(0));
        // Slot 1 of region (1,2) is chunk (33,64).
        write_mcc(dir.path(), 33, 64, &stone).unwrap();

        let changed = normalize(&path, dir.path(), 1, 2).unwrap();
        assert_eq!(changed, 1023);
        let after = read_slots(&path).unwrap();
        assert_eq!(
            after[1],
            Slot::Inline {
                version: 2,
                data: stone.clone()
            }
        );
        assert_eq!(after[2], Slot::Empty);
        assert!(!mcc_file(dir.path(), 33, 64).exists());
    }

    #[test]
    fn linking_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, dst) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("dst"),
        );
        fs::write(&a, b"first").unwrap();
        fs::write(&b, b"second").unwrap();
        assert!(link_no_replace(&a, &dst).unwrap());
        assert!(!link_no_replace(&b, &dst).unwrap());
        assert_eq!(fs::read(&dst).unwrap(), b"first");
        assert!(!a.exists() && b.exists());
    }

    #[test]
    fn void_chunks_are_told_from_real_ones() {
        let air = zlib_chunk("minecraft:full", "minecraft:air");
        let stone = zlib_chunk("minecraft:full", "minecraft:stone");
        let proto = zlib_chunk("minecraft:structure_starts", "minecraft:stone");
        assert!(is_void_chunk(2, &air));
        assert!(!is_void_chunk(2, &stone));
        assert!(is_void_chunk(2, &proto));
        assert!(!is_void_chunk(2, b"not zlib"));
    }

    #[test]
    fn fill_replaces_gaps_and_void_but_keeps_real_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let (live, staged) = (dir.path().join("live.mca"), dir.path().join("staged.mca"));
        let stone = zlib_chunk("minecraft:full", "minecraft:stone");
        let grass = zlib_chunk("minecraft:full", "minecraft:grass_block");
        let air = zlib_chunk("minecraft:full", "minecraft:air");
        let mut l = vec![Slot::Empty; SLOTS];
        l[0] = Slot::Inline {
            version: 2,
            data: stone.clone(),
        };
        l[1] = Slot::Inline {
            version: 2,
            data: air,
        };
        write_slots(&live, &l).unwrap();
        let s: Vec<Slot> = (0..SLOTS)
            .map(|_| Slot::Inline {
                version: 2,
                data: grass.clone(),
            })
            .collect();
        write_slots(&staged, &s).unwrap();
        assert_eq!(fill_from(&live, &staged, dir.path(), 0, 0).unwrap(), 1023);
        let after = read_slots(&live).unwrap();
        assert_eq!(
            after[0],
            Slot::Inline {
                version: 2,
                data: stone
            }
        );
        assert_eq!(
            after[1],
            Slot::Inline {
                version: 2,
                data: grass.clone()
            }
        );
    }

    #[test]
    fn fill_takes_stubs_and_clear_void_empties_server_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let (live, staged) = (dir.path().join("live.mca"), dir.path().join("staged.mca"));
        let grass = zlib_chunk("minecraft:full", "minecraft:grass_block");
        let air = zlib_chunk("minecraft:full", "minecraft:air");
        let mut l = vec![Slot::Empty; SLOTS];
        l[0] = Slot::External { version: 2 };
        l[1] = Slot::Inline {
            version: 2,
            data: air,
        };
        l[2] = Slot::External { version: 2 };
        write_slots(&live, &l).unwrap();
        // Chunk (0,0) is a real one too large to inline; (2,0) is a stub nothing reached.
        write_mcc(dir.path(), 0, 0, &grass).unwrap();
        assert_eq!(clear_void(&live).unwrap(), 1);
        assert_eq!(read_slots(&live).unwrap()[1], Slot::Empty);
        let s: Vec<Slot> = (0..SLOTS)
            .map(|_| Slot::Inline {
                version: 2,
                data: grass.clone(),
            })
            .collect();
        write_slots(&staged, &s).unwrap();
        assert_eq!(fill_from(&live, &staged, dir.path(), 0, 0).unwrap(), 1023);
        let after = read_slots(&live).unwrap();
        assert_eq!(after[0], Slot::External { version: 2 });
        assert!(matches!(after[2], Slot::Inline { .. }));
        assert!(mcc_file(dir.path(), 0, 0).exists());
    }

    #[test]
    fn region_names_parse() {
        assert_eq!(parse_region_name("r.-3.12.mca"), Some((-3, 12)));
        assert_eq!(parse_region_name("r.1.2.mcc"), None);
        assert_eq!(parse_region_name("c.1.2.mcc"), None);
    }
}
