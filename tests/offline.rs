//! `--offline` against an empty cache stops at once and says what is missing.
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn offline_with_empty_cache_fails_fast() {
    let tmp = tempfile::tempdir().unwrap();
    let (cache, out) = (tmp.path().join("cache"), tmp.path().join("out"));
    std::fs::create_dir_all(&out).unwrap();
    let started = Instant::now();
    let run = Command::new(env!("CARGO_BIN_EXE_arnis"))
        .env("ARNIS_CACHE_ROOT", &cache)
        .env("ARNIS_NO_UPDATE_CHECK", "1")
        .args(["--offline", "--bbox", "44.4450,26.0950,44.4480,26.1030"])
        .arg("--output-dir")
        .arg(&out)
        .output()
        .expect("run arnis");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("OSM") || stderr.contains("elevation"),
        "stderr names nothing: {stderr}"
    );
    // No region file was written, and the world made for the run is gone.
    let mut stack = vec![out];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                assert_ne!(p.extension().and_then(|x| x.to_str()), Some("mca"), "{p:?}");
            }
        }
    }
}

const PARITY_BBOX: &str = "44.4450,26.0950,44.4480,26.1030";

fn arnis(cache: &std::path::Path, args: &[&str], out: Option<&std::path::Path>) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arnis"));
    cmd.env("ARNIS_CACHE_ROOT", cache)
        .env("ARNIS_NO_UPDATE_CHECK", "1")
        .args(args)
        .args(["--bbox", PARITY_BBOX]);
    if let Some(out) = out {
        cmd.arg("--output-dir").arg(out);
    }
    let run = cmd.output().expect("run arnis");
    assert!(
        run.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

fn world_of(out: &std::path::Path) -> std::path::PathBuf {
    let mut stack = vec![out.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.file_name().is_some_and(|n| n == "region") {
                return dir;
            }
            if p.is_dir() {
                stack.push(p);
            }
        }
    }
    panic!("no world under {}", out.display());
}

/// Needs the network, python with nbtlib and `CHUNKDIFF=<path to chunkdiff.py>`: fills a fresh
/// cache with `--prewarm`, then builds the parity area with `--offline` and again online from the
/// same cache; the two worlds must not differ in a chunk.
#[test]
#[ignore]
fn prewarm_then_offline_equals_online() {
    let chunkdiff = std::env::var("CHUNKDIFF").expect("set CHUNKDIFF to chunkdiff.py");
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    let (offline, online) = (tmp.path().join("offline"), tmp.path().join("online"));
    for dir in [&offline, &online] {
        std::fs::create_dir_all(dir).unwrap();
    }
    arnis(&cache, &["--prewarm"], None);
    arnis(&cache, &["--offline"], Some(&offline));
    arnis(&cache, &[], Some(&online));
    let out = Command::new("python")
        .arg(&chunkdiff)
        .arg(world_of(&offline))
        .arg(world_of(&online))
        .output()
        .expect("run chunkdiff");
    let text = String::from_utf8_lossy(&out.stdout);
    // The number after `key=` or `key: `.
    let num = |key: &str| -> u64 {
        let rest = &text[text.find(key).unwrap_or_else(|| panic!("{key} in {text}")) + key.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().unwrap()
    };
    // Chunks that differ only in the clock (LastUpdate, InhabitedTime) are equal.
    let real = num("only_A=") + num("only_B=") + num("differing=")
        - num("after dropping ['InhabitedTime', 'LastUpdate']: ");
    assert_eq!(real, 0, "{text}");
}
