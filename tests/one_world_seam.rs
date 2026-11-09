//! Builds the parity area as one One World area and as two, and counts the blocks that differ.
//! Needs network, so run it explicitly:
//! `cargo test --release --no-default-features --test one_world_seam -- --ignored`
use fastanvil::{Chunk, JavaChunk, Region};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn arnis(out: &Path, bbox: &str) {
    let status = Command::new(env!("CARGO_BIN_EXE_arnis"))
        .args(["--one-world", "--world-name", "W", "--output-dir"])
        .arg(out)
        .args(["--bbox", bbox])
        .status()
        .expect("run arnis");
    assert!(status.success(), "arnis failed on {bbox}");
}

/// Every chunk the world has written, by chunk position.
fn chunks(world: &Path) -> HashMap<(i64, i64), JavaChunk> {
    let mut out = HashMap::new();
    for entry in std::fs::read_dir(world.join("region")).expect("read region folder") {
        let path = entry.expect("region entry").path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let ["r", rx, rz, "mca"] = name.split('.').collect::<Vec<_>>()[..] else {
            continue;
        };
        let (rx, rz): (i64, i64) = (rx.parse().unwrap(), rz.parse().unwrap());
        let file = std::fs::File::open(&path).expect("open region");
        let mut region = Region::from_stream(file).expect("read region");
        for chunk in region.iter() {
            let chunk = chunk.expect("read chunk");
            let parsed = JavaChunk::from_bytes(&chunk.data).expect("parse chunk");
            out.insert((rx * 32 + chunk.x as i64, rz * 32 + chunk.z as i64), parsed);
        }
    }
    out
}

fn differing_blocks(a: &Path, b: &Path) -> u64 {
    let (a, b) = (chunks(a), chunks(b));
    let mut keys: Vec<_> = a.keys().chain(b.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    let mut differing = 0;
    for key in keys {
        let (ca, cb) = (a.get(&key), b.get(&key));
        let ys = [ca, cb].into_iter().flatten().map(|c| c.y_range());
        let (y0, y1) = ys.fold((isize::MAX, isize::MIN), |(lo, hi), r| {
            (lo.min(r.start), hi.max(r.end))
        });
        for x in 0..16 {
            for z in 0..16 {
                for y in y0..y1 {
                    differing += u64::from(block(ca, x, y, z) != block(cb, x, y, z));
                }
            }
        }
    }
    differing
}

/// The block with its properties, air where the chunk or section is missing.
fn block(chunk: Option<&JavaChunk>, x: usize, y: isize, z: usize) -> &str {
    chunk
        .and_then(|c| c.block(x, y, z))
        .map_or("minecraft:air|", |b| b.encoded_description())
}

#[test]
#[ignore]
fn split_equals_single_on_parity_area() {
    let tmp = std::env::temp_dir().join("arnis-seam-test");
    let _ = std::fs::remove_dir_all(&tmp);
    let (single, split) = (tmp.join("single"), tmp.join("split"));
    for w in [&single, &split] {
        arnis(w, "44.4464,26.0988,44.4466,26.0992"); // seed fixes the frame
    }
    arnis(&single, "44.4450,26.0950,44.4480,26.1030");
    arnis(&split, "44.4450,26.0950,44.4480,26.0990");
    arnis(&split, "44.4450,26.0990,44.4480,26.1030");
    let n = differing_blocks(&single.join("W"), &split.join("W"));
    // About 1,000 differ, all trees, grass and facade picks; with the floors split at the
    // seam it was about 4,500. The headroom is for OpenStreetMap edits to the area.
    assert!(n <= 2000, "{n} blocks differ between single and split");
}
