//! The bits of RocksDB's on-disk format that voxy needs to open a database.
//!
//! Voxy stores its LOD sections in a RocksDB with two column families. Linking
//! librocksdb just to seed a fresh database would drag a bundled C++ build into
//! every Arnis release, so instead we emit the files RocksDB itself leaves
//! behind after a clean create plus one batch of writes:
//!
//! ```text
//! CURRENT            -> "MANIFEST-000005\n"
//! MANIFEST-000005    -> version edits declaring the comparator and both CFs
//! 000004.log         -> the write-ahead log holding every put
//! IDENTITY           -> the db id, matching the manifest's kDbId record
//! ```
//!
//! There are no SST files: RocksDB replays the WAL on the first open and
//! compacts it into tables itself. That is exactly the state a freshly-joined
//! voxy world is in, so it is a shape the mod already exercises.

use std::io::Write;

/// Log files (WAL and MANIFEST alike) are framed in 32 KiB blocks.
pub(crate) const BLOCK_SIZE: usize = 32768;
/// checksum (4) + length (2) + type (1).
const HEADER_SIZE: usize = 7;

const K_FULL: u8 = 1;
const K_FIRST: u8 = 2;
const K_MIDDLE: u8 = 3;
const K_LAST: u8 = 4;

/// `kTypeColumnFamilyValue` - a put addressed to a non-default column family.
const K_TYPE_COLUMN_FAMILY_VALUE: u8 = 0x05;

const CRC32C: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISCSI);

/// RocksDB stores a rotated CRC so a stored checksum can never be mistaken for
/// the checksum of a buffer that happens to contain it.
fn mask_crc(crc: u32) -> u32 {
    crc.rotate_right(15).wrapping_add(0xa282_ead8)
}

/// Appends records to a RocksDB/LevelDB-style log file.
pub(crate) struct LogWriter<W: Write> {
    out: W,
    block_offset: usize,
}

impl<W: Write> LogWriter<W> {
    pub(crate) fn new(out: W) -> Self {
        Self {
            out,
            block_offset: 0,
        }
    }

    /// Writes one logical record, fragmenting it across blocks when needed.
    pub(crate) fn add_record(&mut self, mut payload: &[u8]) -> std::io::Result<()> {
        let mut first = true;
        loop {
            // A header must not straddle a block boundary; pad the tail instead.
            let left = BLOCK_SIZE - self.block_offset;
            if left < HEADER_SIZE {
                if left > 0 {
                    self.out.write_all(&[0u8; HEADER_SIZE][..left])?;
                }
                self.block_offset = 0;
            }

            // `avail` can be 0 when a record starts with exactly a header's
            // worth of room left. RocksDB's own writer emits a zero-length
            // FIRST fragment here rather than skipping the block, and its
            // reader treats that as the start of a fragmented record, so match
            // it exactly. See `records_can_start_with_only_a_header_left`.
            let avail = BLOCK_SIZE - self.block_offset - HEADER_SIZE;
            let take = payload.len().min(avail);
            let last = take == payload.len();
            let kind = match (first, last) {
                (true, true) => K_FULL,
                (true, false) => K_FIRST,
                (false, true) => K_LAST,
                (false, false) => K_MIDDLE,
            };
            self.emit(kind, &payload[..take])?;
            payload = &payload[take..];
            first = false;
            if last {
                return Ok(());
            }
        }
    }

    fn emit(&mut self, kind: u8, data: &[u8]) -> std::io::Result<()> {
        let mut digest = CRC32C.digest();
        digest.update(&[kind]);
        digest.update(data);
        let crc = mask_crc(digest.finalize());

        let mut header = [0u8; HEADER_SIZE];
        header[0..4].copy_from_slice(&crc.to_le_bytes());
        header[4..6].copy_from_slice(&(data.len() as u16).to_le_bytes());
        header[6] = kind;

        self.out.write_all(&header)?;
        self.out.write_all(data)?;
        self.block_offset += HEADER_SIZE + data.len();
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

fn put_varint32(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_varint64(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_length_prefixed(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varint32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

/// A `WriteBatch` holding a single put into a named column family. One put per
/// batch costs 12 bytes of framing; across a few thousand sections that is
/// noise, and it keeps the sequence-number bookkeeping trivial.
pub(crate) fn write_batch_put(seq: u64, cf: u32, key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(24 + key.len() + value.len());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // record count
    out.push(K_TYPE_COLUMN_FAMILY_VALUE);
    put_varint32(&mut out, cf);
    put_length_prefixed(&mut out, key);
    put_length_prefixed(&mut out, value);
    out
}

// VersionEdit record tags. `kTagSafeIgnoreMask` (1 << 13) marks tags a reader
// may skip, which is where kDbId and the timestamp flag live.
const TAG_COMPARATOR: u32 = 1;
const TAG_LOG_NUMBER: u32 = 2;
const TAG_NEXT_FILE_NUMBER: u32 = 3;
const TAG_LAST_SEQUENCE: u32 = 4;
const TAG_PREV_LOG_NUMBER: u32 = 9;
const TAG_COLUMN_FAMILY: u32 = 200;
const TAG_COLUMN_FAMILY_ADD: u32 = 201;
const TAG_DB_ID: u32 = 8193;
const TAG_PERSIST_USER_DEFINED_TIMESTAMPS: u32 = 8201;

const COMPARATOR: &[u8] = b"leveldb.BytewiseComparator";

/// Column family ids, in the order voxy opens them.
pub(crate) const CF_WORLD_SECTIONS: u32 = 1;
pub(crate) const CF_ID_MAPPINGS: u32 = 2;

fn edit_comparator(out: &mut Vec<u8>) {
    put_varint32(out, TAG_COMPARATOR);
    put_length_prefixed(out, COMPARATOR);
}

fn edit_persist_udt(out: &mut Vec<u8>) {
    put_varint32(out, TAG_PERSIST_USER_DEFINED_TIMESTAMPS);
    put_length_prefixed(out, &[1]);
}

fn edit_u64(out: &mut Vec<u8>, tag: u32, value: u64) {
    put_varint32(out, tag);
    put_varint64(out, value);
}

/// The MANIFEST for a fresh two-column-family database.
///
/// `log_number` is the WAL both column families recover from, and
/// `next_file_number` must exceed every file number already on disk.
/// `last_sequence` stays 0: recovery replays the WAL and derives the real
/// sequence from it.
pub(crate) fn manifest_bytes(db_id: &str, log_number: u64, next_file_number: u64) -> Vec<u8> {
    let mut records: Vec<Vec<u8>> = Vec::new();

    let mut r = Vec::new();
    put_varint32(&mut r, TAG_DB_ID);
    put_length_prefixed(&mut r, db_id.as_bytes());
    records.push(r);

    // RocksDB writes an empty edit here as a snapshot separator.
    records.push(Vec::new());

    let mut r = Vec::new();
    edit_comparator(&mut r);
    edit_persist_udt(&mut r);
    records.push(r);

    // The default column family, which voxy never writes to.
    let mut r = Vec::new();
    edit_u64(&mut r, TAG_LOG_NUMBER, 0);
    edit_u64(&mut r, TAG_LAST_SEQUENCE, 0);
    records.push(r);

    let mut r = Vec::new();
    edit_u64(&mut r, TAG_PREV_LOG_NUMBER, 0);
    edit_u64(&mut r, TAG_NEXT_FILE_NUMBER, next_file_number);
    edit_u64(&mut r, TAG_LAST_SEQUENCE, 0);
    records.push(r);

    for (id, name) in [
        (CF_WORLD_SECTIONS as u64, "world_sections"),
        (CF_ID_MAPPINGS as u64, "id_mappings"),
    ] {
        let mut r = Vec::new();
        edit_comparator(&mut r);
        edit_u64(&mut r, TAG_LOG_NUMBER, log_number);
        edit_u64(&mut r, TAG_NEXT_FILE_NUMBER, next_file_number);
        edit_u64(&mut r, TAG_LAST_SEQUENCE, 0);
        edit_u64(&mut r, TAG_COLUMN_FAMILY, id);
        put_varint32(&mut r, TAG_COLUMN_FAMILY_ADD);
        put_length_prefixed(&mut r, name.as_bytes());
        edit_persist_udt(&mut r);
        records.push(r);
    }

    let mut out = Vec::new();
    {
        let mut w = LogWriter::new(&mut out);
        for record in &records {
            w.add_record(record).expect("writing to a Vec cannot fail");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_matches_the_standard_vectors() {
        assert_eq!(CRC32C.checksum(b"123456789"), 0xE306_9283);
        assert_eq!(CRC32C.checksum(b""), 0);
    }

    #[test]
    fn records_fragment_on_block_boundaries() {
        let payload = vec![0xABu8; 40_000];
        let mut out = Vec::new();
        LogWriter::new(&mut out).add_record(&payload).unwrap();

        // One FIRST filling the rest of block 0, one LAST with the remainder.
        let first_len = BLOCK_SIZE - HEADER_SIZE;
        assert_eq!(
            out.len(),
            BLOCK_SIZE + HEADER_SIZE + (payload.len() - first_len)
        );
        assert_eq!(out[6], K_FIRST);
        assert_eq!(
            u16::from_le_bytes([out[4], out[5]]) as usize,
            first_len,
            "the first fragment must fill the block exactly"
        );
        assert_eq!(out[BLOCK_SIZE + 6], K_LAST);
        let last_len = u16::from_le_bytes([out[BLOCK_SIZE + 4], out[BLOCK_SIZE + 5]]) as usize;
        assert_eq!(first_len + last_len, payload.len());
    }

    /// The awkward alignment: a record that begins with exactly `HEADER_SIZE`
    /// bytes left in the block. RocksDB's `log_writer.cc` takes
    /// `fragment_length = min(left, avail)` with no special case, so it emits an
    /// empty FIRST and continues in the next block; anything else would produce
    /// a log its reader frames differently from one RocksDB wrote itself.
    #[test]
    fn records_can_start_with_only_a_header_left() {
        let mut out = Vec::new();
        {
            let mut w = LogWriter::new(&mut out);
            // Leaves the block with room for a header and nothing else.
            w.add_record(&vec![1u8; BLOCK_SIZE - 2 * HEADER_SIZE])
                .unwrap();
            w.add_record(b"tail").unwrap();
        }
        assert_eq!(out.len(), BLOCK_SIZE + HEADER_SIZE + 4);

        let head = BLOCK_SIZE - HEADER_SIZE;
        assert_eq!(out[head + 6], K_FIRST);
        assert_eq!(u16::from_le_bytes([out[head + 4], out[head + 5]]), 0);
        assert_eq!(out[BLOCK_SIZE + 6], K_LAST);
        assert_eq!(
            u16::from_le_bytes([out[BLOCK_SIZE + 4], out[BLOCK_SIZE + 5]]),
            4
        );
        assert_eq!(&out[BLOCK_SIZE + HEADER_SIZE..], b"tail");

        // The empty fragment carries a real checksum, and the record reassembles.
        let records = read_back(&out);
        assert_eq!(records.len(), 2);
        assert_eq!(records[1], b"tail");
    }

    /// Reassembles logical records, the way RocksDB's `log_reader.cc` does.
    fn read_back(log: &[u8]) -> Vec<Vec<u8>> {
        let mut records = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut off = 0usize;
        while off + HEADER_SIZE <= log.len() {
            let block_end = ((off / BLOCK_SIZE) + 1) * BLOCK_SIZE;
            if block_end - off < HEADER_SIZE {
                off = block_end;
                continue;
            }
            let stored = u32::from_le_bytes(log[off..off + 4].try_into().unwrap());
            let len = u16::from_le_bytes(log[off + 4..off + 6].try_into().unwrap()) as usize;
            let kind = log[off + 6];
            if kind == 0 && len == 0 {
                off = block_end;
                continue;
            }
            let mut digest = CRC32C.digest();
            digest.update(&log[off + 6..off + 7]);
            digest.update(&log[off + HEADER_SIZE..off + HEADER_SIZE + len]);
            assert_eq!(stored, mask_crc(digest.finalize()), "bad crc at {off}");

            let fragment = &log[off + HEADER_SIZE..off + HEADER_SIZE + len];
            match kind {
                K_FULL => records.push(fragment.to_vec()),
                K_FIRST => pending = fragment.to_vec(),
                K_MIDDLE => pending.extend_from_slice(fragment),
                K_LAST => {
                    pending.extend_from_slice(fragment);
                    records.push(std::mem::take(&mut pending));
                }
                other => panic!("unexpected record type {other}"),
            }
            off += HEADER_SIZE + len;
        }
        assert!(pending.is_empty(), "log ends mid-record");
        records
    }

    /// Every record's stored checksum is the masked CRC32C of type byte + fragment.
    #[test]
    fn record_checksums_verify() {
        let mut out = Vec::new();
        {
            let mut w = LogWriter::new(&mut out);
            w.add_record(b"hello").unwrap();
            w.add_record(&vec![7u8; 33_000]).unwrap();
        }
        let mut off = 0usize;
        let mut records = 0usize;
        while off + HEADER_SIZE <= out.len() {
            let block_end = ((off / BLOCK_SIZE) + 1) * BLOCK_SIZE;
            if block_end - off < HEADER_SIZE {
                off = block_end;
                continue;
            }
            let stored = u32::from_le_bytes(out[off..off + 4].try_into().unwrap());
            let len = u16::from_le_bytes(out[off + 4..off + 6].try_into().unwrap()) as usize;
            let kind = out[off + 6];
            if kind == 0 && len == 0 {
                off = block_end;
                continue;
            }
            let mut d = CRC32C.digest();
            d.update(&out[off + 6..off + 7]);
            d.update(&out[off + HEADER_SIZE..off + HEADER_SIZE + len]);
            assert_eq!(stored, mask_crc(d.finalize()), "bad crc at {off}");
            records += 1;
            off += HEADER_SIZE + len;
        }
        assert_eq!(records, 3); // FULL, FIRST, LAST
    }

    /// Golden test against the MANIFEST RocksDB 10.1.3 wrote for a real voxy
    /// world. If this drifts, the database we emit is no longer the shape the
    /// mod's own RocksDB build produces.
    #[test]
    fn manifest_matches_a_real_rocksdb_manifest() {
        let golden = include_bytes!("../../assets/voxy/MANIFEST.golden");
        let got = manifest_bytes("4eea700b-a854-11f1-b632-001fb524f051", 4, 6);
        assert_eq!(got, golden.to_vec());
    }

    /// A write batch decodes back to the put it was built from.
    #[test]
    fn write_batch_round_trips() {
        let batch = write_batch_put(7, CF_WORLD_SECTIONS, b"12345678", b"payload");
        assert_eq!(u64::from_le_bytes(batch[0..8].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(batch[8..12].try_into().unwrap()), 1);
        assert_eq!(batch[12], K_TYPE_COLUMN_FAMILY_VALUE);
        assert_eq!(batch[13], CF_WORLD_SECTIONS as u8);
        assert_eq!(batch[14], 8);
        assert_eq!(&batch[15..23], b"12345678");
        assert_eq!(batch[23], 7);
        assert_eq!(&batch[24..31], b"payload");
    }
}
