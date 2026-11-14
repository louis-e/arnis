//! EXPERIMENTAL: Leaf / Luminol **B_Linear v3** region container (`r.X.Z.b_linear`),
//! selected by `--region-format blinear`. Off by default; Anvil `.mca` stays the format.
//!
//! The same uncompressed chunk NBT the Anvil writer hands to zlib, framed differently:
//! a region's 1024 chunks are grouped into 16 buckets of 64 and each bucket is one zstd
//! frame. Readable by Leaf 1.21.11 (June 2026 builds) and newer and all Leaf 26.x
//! (`misc.region-format.format-name: B_LINEAR`), and by Meld's `region-convert`.
//! Paper, older Leaf and the vanilla client cannot open it.
//!
//! Wire format, all integers big-endian, as read by Leaf's
//! `me.earthme.luminol.data.BufferedLinearRegionFile` (layout ported from Meld's fork,
//! `src/world_editor/blinear.rs`, and its `region-convert` reader):
//!
//! ```text
//! [0,8)     i64  superblock = -0x2008_1225_0269
//! [8]       u8   version    = 3
//! [9]       u8   zstd level (informational)
//! [10,14)   u32  xxh32 seed = 0x0721 (Leaf hardcodes it; region-convert reads it)
//! [14,142)  16 x u64 absolute offset of each bucket record, 0 = bucket absent
//! [142,EOF) bucket records: i32 rawLen | i32 compressedLen | zstd frame
//! ```
//!
//! A decompressed bucket is 64 slots in ascending chunk index (`x + z * 32`), each an
//! `i32` length (0 = absent) and then that many bytes:
//! `i32 nbtLen | i64 timestampMillis | u32 xxh32(nbt, 0x0721) | nbt`.
//! Leaf checks the superblock, the version and every chunk hash. No footer.

use rayon::prelude::*;
use std::path::{Path, PathBuf};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const SUPERBLOCK: i64 = -0x0000_2008_1225_0269;
const VERSION: u8 = 3;
const HASH_SEED: u32 = 0x0721;
const BUCKET_COUNT: usize = 16;
const BUCKET_SIZE: usize = 64;
const CHUNKS_PER_REGION: usize = BUCKET_COUNT * BUCKET_SIZE;
const HEADER_SIZE: usize = 14;
const DATA_START: usize = HEADER_SIZE + BUCKET_COUNT * 8;

/// Holds one region's chunk NBT until [`finish`](Self::finish). Buckets compress as
/// units, so nothing can be written before the region is complete.
pub(crate) struct BlinearRegionWriter {
    chunks: [Option<Vec<u8>>; CHUNKS_PER_REGION],
    level: i32,
    out_path: PathBuf,
}

impl BlinearRegionWriter {
    pub(crate) fn create(
        world_dir: &Path,
        region_x: i32,
        region_z: i32,
        level: i32,
    ) -> Result<Self, BoxError> {
        let region_dir = world_dir.join("region");
        std::fs::create_dir_all(&region_dir)?;
        Ok(Self {
            chunks: std::array::from_fn(|_| None),
            level: level.clamp(1, 22),
            out_path: region_dir.join(format!("r.{region_x}.{region_z}.b_linear")),
        })
    }

    /// Same index convention as `fastanvil::Region::write_chunk`.
    pub(crate) fn write_chunk(&mut self, chunk_x: usize, chunk_z: usize, nbt: &[u8]) {
        self.chunks[(chunk_x & 31) + (chunk_z & 31) * 32] = Some(nbt.to_vec());
    }

    /// Compress and publish via a sibling temp + rename, so a reader never sees a torn
    /// file. The temp (`r.X.Z.tmp<pid>`) does not match `r.*.b_linear`.
    pub(crate) fn finish(self) -> Result<(), BoxError> {
        // The in-memory Anvil region leaves every header timestamp 0, so these are 0 too.
        let bytes = encode(&self.chunks, self.level, &[0; CHUNKS_PER_REGION]);
        crate::world_utils::replace_file_atomically(&self.out_path, &bytes)?;
        Ok(())
    }
}

/// One region as a B_Linear v3 file. `timestamps` are the Anvil header's seconds.
pub(crate) fn encode(
    chunks: &[Option<Vec<u8>>; CHUNKS_PER_REGION],
    level: i32,
    timestamps: &[u32; CHUNKS_PER_REGION],
) -> Vec<u8> {
    // Buckets are independent: compress them in parallel.
    let frames: Vec<_> = chunks
        .par_chunks(BUCKET_SIZE)
        .zip(timestamps.par_chunks(BUCKET_SIZE))
        .map(|(bucket, stamps)| encode_bucket(bucket, stamps, level))
        .collect();

    let mut out = Vec::new();
    out.extend_from_slice(&SUPERBLOCK.to_be_bytes());
    out.push(VERSION);
    out.push(level as u8);
    out.extend_from_slice(&HASH_SEED.to_be_bytes());
    out.resize(DATA_START, 0);
    for (i, frame) in frames.into_iter().enumerate() {
        let Some((raw_len, compressed)) = frame else {
            continue; // all 64 slots empty: offset stays 0
        };
        let at = HEADER_SIZE + i * 8;
        let here = out.len() as u64;
        out[at..at + 8].copy_from_slice(&here.to_be_bytes());
        out.extend_from_slice(&(raw_len as i32).to_be_bytes());
        out.extend_from_slice(&(compressed.len() as i32).to_be_bytes());
        out.extend_from_slice(&compressed);
    }
    out
}

/// `(raw length, zstd frame)` of one bucket, or `None` when it holds no chunk.
fn encode_bucket(
    bucket: &[Option<Vec<u8>>],
    stamps: &[u32],
    level: i32,
) -> Option<(usize, Vec<u8>)> {
    if bucket.iter().all(Option::is_none) {
        return None;
    }
    let mut raw = Vec::new();
    for (chunk, stamp) in bucket.iter().zip(stamps) {
        let Some(nbt) = chunk else {
            raw.extend_from_slice(&0i32.to_be_bytes());
            continue;
        };
        // Anvil caps a chunk near 1 MiB, far below i32::MAX.
        let nbt_len = nbt.len() as i32;
        raw.extend_from_slice(&(nbt_len + 16).to_be_bytes());
        raw.extend_from_slice(&nbt_len.to_be_bytes());
        raw.extend_from_slice(&(i64::from(*stamp) * 1000).to_be_bytes());
        raw.extend_from_slice(&xxh32(nbt, HASH_SEED).to_be_bytes());
        raw.extend_from_slice(nbt);
    }
    let frame = zstd::bulk::compress(&raw, level).expect("zstd compresses in memory");
    Some((raw.len(), frame))
}

/// XXH32 (one-shot), the chunk checksum Leaf verifies.
fn xxh32(input: &[u8], seed: u32) -> u32 {
    twox_hash::XxHash32::oneshot(seed, input)
}

/// Reader for tests: makes the checks Leaf makes (superblock, version, chunk hash)
/// and returns the 1024 slots' NBT.
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8]) -> Vec<Option<Vec<u8>>> {
    let be32 = |b: &[u8], at: usize| i32::from_be_bytes(b[at..at + 4].try_into().unwrap());
    assert_eq!(&bytes[0..8], &SUPERBLOCK.to_be_bytes());
    assert_eq!(bytes[8], VERSION);
    let seed = u32::from_be_bytes(bytes[10..14].try_into().unwrap());
    let mut slots = vec![None; CHUNKS_PER_REGION];
    for b in 0..BUCKET_COUNT {
        let at = HEADER_SIZE + b * 8;
        let offset = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
        if offset == 0 {
            continue;
        }
        let raw_len = be32(bytes, offset) as usize;
        let comp_len = be32(bytes, offset + 4) as usize;
        let raw = zstd::bulk::decompress(&bytes[offset + 8..offset + 8 + comp_len], raw_len)
            .expect("bucket decompresses");
        assert_eq!(raw.len(), raw_len);
        let mut cursor = 0;
        for slot in 0..BUCKET_SIZE {
            let n = be32(&raw, cursor) as usize;
            cursor += 4;
            if n == 0 {
                continue;
            }
            let nbt_len = be32(&raw, cursor) as usize;
            assert_eq!(n, nbt_len + 16);
            let hash = u32::from_be_bytes(raw[cursor + 12..cursor + 16].try_into().unwrap());
            let nbt = &raw[cursor + 16..cursor + n];
            assert_eq!(xxh32(nbt, seed), hash, "chunk hash");
            slots[b * BUCKET_SIZE + slot] = Some(nbt.to_vec());
            cursor += n;
        }
        assert_eq!(cursor, raw.len(), "bucket fully consumed");
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none_chunks() -> [Option<Vec<u8>>; CHUNKS_PER_REGION] {
        std::array::from_fn(|_| None)
    }

    #[test]
    fn xxh32_matches_reference_vectors() {
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"a", 0), 0x550D_7456);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
        assert_eq!(
            xxh32(b"Nobody inspects the spammish repetition", 0),
            0xE229_3B2F
        );
    }

    #[test]
    fn header_matches_the_leaf_contract() {
        let bytes = encode(&none_chunks(), 9, &[0; CHUNKS_PER_REGION]);
        assert_eq!(bytes.len(), 142, "empty region is header + zero table");
        assert_eq!(
            &bytes[0..8],
            &[0xFF, 0xFF, 0xDF, 0xF7, 0xED, 0xDA, 0xFD, 0x97]
        );
        assert_eq!(bytes[8], 3);
        assert_eq!(bytes[9], 9);
        assert_eq!(&bytes[10..14], &[0x00, 0x00, 0x07, 0x21]);
        assert!(bytes[14..].iter().all(|&b| b == 0));
    }

    /// Chunks at slots 0, 63, 64, 513, 1023: both ends of a bucket, empty buckets between.
    fn fixture() -> [Option<Vec<u8>>; CHUNKS_PER_REGION] {
        let mut chunks = none_chunks();
        for i in [0usize, 63, 64, 513, 1023] {
            let array: Vec<u8> = (0..300 * (i % 5 + 1)).map(|j| (i * 7 + j) as u8).collect();
            let mut nbt = vec![0x0A, 0, 0, 0x07, 0, 1, b'a'];
            nbt.extend_from_slice(&(array.len() as i32).to_be_bytes());
            nbt.extend_from_slice(&array);
            nbt.push(0);
            chunks[i] = Some(nbt);
        }
        chunks
    }

    #[test]
    fn roundtrip_decode_encode() {
        let chunks = fixture();
        let bytes = encode(&chunks, 6, &[7; CHUNKS_PER_REGION]);
        assert_eq!(decode(&bytes), chunks.to_vec());
    }

    #[test]
    fn writer_publishes_one_file_with_chunks_in_their_slots() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = BlinearRegionWriter::create(dir.path(), -3, 7, 6).unwrap();
        let payloads = [
            (9, 0, vec![1u8; 3]),
            (31, 1, vec![0xAB; 5000]),
            (0, 2, b"bucket one".to_vec()),
            (31, 31, b"last".to_vec()),
        ];
        for (x, z, nbt) in &payloads {
            w.write_chunk(*x, *z, nbt);
        }
        w.finish().unwrap();

        let region = dir.path().join("region");
        let names: Vec<_> = std::fs::read_dir(&region)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["r.-3.7.b_linear"], "published, no temp left");
        let slots = decode(&std::fs::read(region.join("r.-3.7.b_linear")).unwrap());
        for (x, z, nbt) in &payloads {
            assert_eq!(slots[x + z * 32].as_ref(), Some(nbt));
        }
        assert_eq!(slots.iter().flatten().count(), payloads.len());
    }
}
