//! Run explicitly: `SEAM_BLOCKDIFF=<path to blockdiff.py> cargo test --release --test one_world_seam -- --ignored`.
//! Needs network and python with nbtlib; builds the parity area as one area and as two.
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

#[test]
#[ignore]
fn split_equals_single_on_parity_area() {
    let blockdiff = std::env::var("SEAM_BLOCKDIFF").expect("set SEAM_BLOCKDIFF to blockdiff.py");
    let tmp = std::env::temp_dir().join("arnis-seam-test");
    let _ = std::fs::remove_dir_all(&tmp);
    let (single, split) = (tmp.join("single"), tmp.join("split"));
    for w in [&single, &split] {
        arnis(w, "44.4464,26.0988,44.4466,26.0992"); // seed fixes the frame
    }
    arnis(&single, "44.4450,26.0950,44.4480,26.1030");
    arnis(&split, "44.4450,26.0950,44.4480,26.0990");
    arnis(&split, "44.4450,26.0990,44.4480,26.1030");
    let out = Command::new("python")
        .arg(&blockdiff)
        .arg(single.join("W"))
        .arg(split.join("W"))
        .output()
        .expect("run blockdiff");
    let text = String::from_utf8_lossy(&out.stdout);
    let n: u64 = text
        .split("differing blocks: ")
        .nth(1)
        .and_then(|s| s.lines().next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("unparsable blockdiff output: {text}"));
    // 788 was the Meld fork's figure; the rest here is vegetation and facade picks, not floors.
    assert!(n <= 1000, "{n} blocks differ between single and split");
}
