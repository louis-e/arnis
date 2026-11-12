//! Local Archive: runs louis-e/arnis-tiles to bake Geofabrik country extracts
//! into a local tile archive folder that `--osm-tiles-url <folder>` reads.
//!
//! arnis-tiles is a separate program: the release bundles it as a Tauri
//! sidecar next to the Arnis executable, and a dev build finds it on PATH or
//! at the path set in the window.

use std::ffi::OsStr;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::transfer::{RateMeter, Transfer};

pub const HOME: &str = "https://github.com/louis-e/arnis-tiles";

const EXE: &str = if cfg!(windows) {
    "arnis-tiles.exe"
} else {
    "arnis-tiles"
};

/// The arnis-tiles executable: next to this one (where the bundle puts the
/// sidecar), then on PATH.
pub fn locate() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    locate_in(exe_dir.as_deref(), std::env::var_os("PATH").as_deref())
}

fn locate_in(exe_dir: Option<&Path>, path: Option<&OsStr>) -> Option<PathBuf> {
    exe_dir
        .map(|d| d.join(EXE))
        .into_iter()
        .chain(
            path.into_iter()
                .flat_map(std::env::split_paths)
                // An empty or relative entry is the working directory, which anyone may fill.
                .filter(|d| d.is_absolute())
                .map(|d| d.join(EXE)),
        )
        .find(|p| p.is_file())
}

/// The default archive folder under the cache root.
pub fn default_folder(root: &Path) -> PathBuf {
    root.join("arnis").join("local-archive")
}

/// One Geofabrik extract `prepare` picked for the selection.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct Extract {
    pub id: String,
    pub name: String,
    /// Download size of the .osm.pbf.
    pub bytes: u64,
    pub url: String,
}

/// The JSON line `arnis-tiles prepare` prints before it downloads anything.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct Prepare {
    pub extracts: Vec<Extract>,
    pub total_bytes: u64,
    /// Sample points of the selection no extract covers (open sea).
    pub uncovered_points: u64,
}

/// The plan out of `prepare`'s output: its last line that parses as one.
pub fn parse_prepare(stdout: &str) -> Option<Prepare> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .find_map(|l| serde_json::from_str(l).ok())
}

/// `exe` with its index cache and scratch kept under `state`, never in the
/// working directory.
fn command(exe: &Path, state: &Path, out: &Path) -> Result<Command, String> {
    std::fs::create_dir_all(state).map_err(|e| format!("{}: {e}", state.display()))?;
    let mut cmd = Command::new(exe);
    cmd.current_dir(state)
        .arg("--cache")
        .arg(state.join("cache"))
        .arg("--work")
        .arg(state.join("work"))
        .arg("--out")
        .arg(out);
    crate::scale::child::prepare(&mut cmd);
    Ok(cmd)
}

/// `prepare --dry-run`: the extracts that cover `bbox` and their sizes. The
/// first call downloads the Geofabrik index into `state`; later ones read it.
pub fn dry_run(
    exe: &Path,
    state: &Path,
    bbox: &crate::coordinate_system::geographic::LLBBox,
) -> Result<Prepare, String> {
    let mut cmd = command(exe, state, &state.join("out"))?;
    // One argument: a south latitude starts with `-` and would read as a flag.
    cmd.args(["prepare", "--dry-run", &format!("--bbox={}", bbox.arg())]);
    let out = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    // With no extract covering the area it prints the empty plan and fails.
    parse_prepare(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        let err = String::from_utf8_lossy(&out.stderr);
        err.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("arnis-tiles printed no plan")
            .trim()
            .to_string()
    })
}

/// Where a bake is, read off arnis-tiles' output lines and, between them,
/// off the files it writes into the folder's `work`.
#[derive(Debug, Default, PartialEq)]
pub struct BakeProgress {
    /// Extract being worked on, 0-based, of `total`.
    index: usize,
    total: usize,
    id: String,
    stage: Stage,
    /// Bytes of the stage done: downloaded, in the chunk store, or in the archive.
    done_bytes: u64,
}

#[derive(Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Planning,
    /// Download percentage.
    Downloading(f64),
    /// osmpbf passes; arnis-tiles prints nothing until they end.
    Baking,
    Writing,
}

impl BakeProgress {
    /// Takes one output line (stdout or stderr).
    pub fn line(&mut self, line: &str) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            // `[2/3] romania (330 MB)` or `[2/3] romania already baked, skipping`
            let Some((count, rest)) = rest.split_once(']') else {
                return;
            };
            let Some((i, n)) = count.split_once('/') else {
                return;
            };
            let (Ok(i), Ok(n)) = (i.parse::<usize>(), n.parse::<usize>()) else {
                return;
            };
            self.index = i.saturating_sub(1);
            self.total = n;
            self.id = rest.split_whitespace().next().unwrap_or("").to_string();
            self.done_bytes = 0;
            self.stage = if rest.contains("already baked") {
                Stage::Writing
            } else {
                Stage::Downloading(0.0)
            };
        } else if let Some(rest) = line.strip_prefix("downloaded ") {
            // `downloaded 120 MB (36%)` while it runs, without a share at the end.
            if let Some(mb) = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<f64>().ok())
            {
                self.done_bytes = (mb * 1e6) as u64;
            }
            self.stage = match rest
                .split_once('(')
                .and_then(|(_, p)| p.split_once('%'))
                .and_then(|(p, _)| p.trim().parse::<f64>().ok())
            {
                Some(p) => Stage::Downloading(p.clamp(0.0, 100.0)),
                None => {
                    self.done_bytes = 0;
                    Stage::Baking
                }
            };
        } else if line.contains(" ways, ") && line.contains(" tiles, ") {
            self.stage = Stage::Writing;
            self.done_bytes = 0;
        }
    }

    /// Reads what the files in `work` (the folder's `work`) and `folder` say
    /// about extract `e`: a download in progress as `<name>.part`, one done as
    /// `<name>` (a download the run started while baking the previous extract
    /// prints nothing), the chunk store growing while it bakes, and the archive
    /// being written. Files older than `since` belong to an earlier run.
    pub fn observe(&mut self, folder: &Path, e: &Extract, since: std::time::SystemTime) {
        if self.id != e.id {
            return;
        }
        let work = folder.join("work");
        let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
        match self.stage {
            Stage::Planning => {}
            Stage::Downloading(_) => {
                let Some(name) = e.url.rsplit('/').next().filter(|n| !n.is_empty()) else {
                    return;
                };
                let pbf = work.join("pbf").join(name);
                if pbf.is_file() {
                    self.stage = Stage::Baking;
                    self.done_bytes = 0;
                } else {
                    let part = size(&work.join("pbf").join(format!("{name}.part")));
                    if part > 0 {
                        self.done_bytes = part;
                        self.stage = Stage::Downloading(
                            (100.0 * part as f64 / e.bytes.max(1) as f64).min(100.0),
                        );
                    }
                }
            }
            Stage::Baking => {
                let store = work.join(format!("chunks-{}.db", e.id));
                self.done_bytes = size(&store) + size(&store.with_extension("db-wal"));
            }
            Stage::Writing => {
                let prefix = format!("{}-", e.id);
                self.done_bytes = std::fs::read_dir(folder)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|f| {
                        let n = f.file_name().to_string_lossy().into_owned();
                        n.starts_with(&prefix) && n.ends_with(".pmtiles")
                    })
                    .filter_map(|f| f.metadata().ok())
                    .filter(|m| m.modified().is_ok_and(|t| t >= since))
                    .map(|m| m.len())
                    .max()
                    .unwrap_or(0);
            }
        }
    }

    /// The transfer stage and how far through it, 0-1. A bake and a write
    /// have no total of their own, so they are measured against the typical
    /// store and archive of an extract of `pbf` bytes, short of the end.
    fn stage_fraction(&self, pbf: u64) -> (crate::transfer::Stage, f64) {
        use crate::transfer::Stage as T;
        let towards =
            |ratio: f64| (self.done_bytes as f64 / (pbf as f64 * ratio).max(1.0)).min(0.95);
        match self.stage {
            Stage::Planning => (T::Download, 0.0),
            Stage::Downloading(p) => (T::Download, p / 100.0),
            Stage::Baking => (T::Bake, towards(crate::data_plan::STORE_PER_PBF)),
            Stage::Writing => (T::Finalize, towards(crate::data_plan::ARCHIVE_PER_PBF)),
        }
    }

    /// Where the bake is, for the panel's bar: `extracts` give the sizes, and
    /// `from` is the job's percent done before arnis-tiles started (its
    /// downloads, when the window fetched them).
    pub fn transfer(
        &self,
        extracts: &[Extract],
        from: f64,
        rate_bps: f64,
        threads: usize,
    ) -> Transfer {
        let pbf = extracts
            .iter()
            .find(|e| e.id == self.id)
            .map_or(0, |e| e.bytes);
        let (stage, frac) = self.stage_fraction(pbf);
        let own = if self.stage == Stage::Planning {
            0.0
        } else {
            crate::transfer::job_percent(self.index, self.total, stage, frac)
        };
        let percent = from + (100.0 - from) * own / 100.0;
        Transfer {
            stage,
            name: self.id.clone(),
            item: self.index + 1,
            items: self.total.max(1),
            done_bytes: self.done_bytes,
            total_bytes: if stage == crate::transfer::Stage::Download {
                pbf
            } else {
                0
            },
            rate_bps,
            threads,
            cpu_pct: crate::transfer::cpu_pct(threads, crate::transfer::cores()),
            // The current extract, and the next one fetched while it bakes.
            downloads: 2,
            percent,
        }
    }

    /// One status line, without the elapsed time.
    pub fn message(&self) -> String {
        let of = format!("{}/{}", self.index + 1, self.total.max(1));
        match self.stage {
            Stage::Planning => "Choosing the extracts to bake...".to_string(),
            Stage::Downloading(p) => format!("Downloading {} ({of}): {p:.0}%", self.id),
            Stage::Baking => format!("Baking {} ({of})...", self.id),
            Stage::Writing => format!("Writing the {} archive ({of})...", self.id),
        }
    }
}

/// The archives a folder lists before a bake, so a stopped bake can leave it
/// as a finished one would: listed archives whole, nothing half written.
#[derive(Debug, Default)]
pub struct Published {
    index: Option<Vec<u8>>,
    files: std::collections::HashSet<String>,
}

const INDEX: &str = "archives.json";

fn pmtiles_in(folder: &Path) -> impl Iterator<Item = String> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".pmtiles"))
}

/// The files `archives.json` in `folder` lists; `None` when it does not parse.
fn listed(folder: &Path) -> Option<std::collections::HashSet<String>> {
    let bytes = match std::fs::read(folder.join(INDEX)) {
        Ok(b) => b,
        Err(_) => return Some(Default::default()),
    };
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(
        v["archives"]
            .as_array()?
            .iter()
            .filter_map(|a| a["file"].as_str().map(str::to_string))
            .collect(),
    )
}

impl Published {
    pub fn take(folder: &Path) -> Self {
        Published {
            index: std::fs::read(folder.join(INDEX)).ok(),
            files: pmtiles_in(folder).collect(),
        }
    }

    /// After a stopped bake: an index cut off mid-write goes back to the one
    /// before, a new archive the index does not list (cut off mid-write) is
    /// removed, and so are half-done downloads. Countries the bake finished
    /// stay, and so does the chunk store arnis-tiles resumes from. Returns
    /// the files removed.
    pub fn tidy(&self, folder: &Path) -> usize {
        let mut removed = 0;
        let listed = listed(folder).unwrap_or_else(|| {
            let index = folder.join(INDEX);
            let _ = match &self.index {
                Some(bytes) => crate::world_utils::replace_file_atomically(&index, bytes),
                None => std::fs::remove_file(&index).map_err(|e| e.to_string()),
            };
            listed(folder).unwrap_or_default()
        });
        let stray = pmtiles_in(folder).filter(|n| !listed.contains(n) && !self.files.contains(n));
        for name in stray.collect::<Vec<_>>() {
            if std::fs::remove_file(folder.join(name)).is_ok() {
                removed += 1;
            }
        }
        let pbf = folder.join("work").join("pbf");
        for e in std::fs::read_dir(pbf).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().ends_with(".part")
                && std::fs::remove_file(e.path()).is_ok()
            {
                removed += 1;
            }
        }
        removed
    }
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// `prepare` for `bbox` into `folder` on `threads` threads, reporting
/// `(percent, message, transfer)` to `report` twice a second, the job's
/// percent going on from `from`. `extracts` is the dry run's plan, for the
/// sizes. Stops the child when `cancel` is set.
/// `Ok(false)` when cancelled; the countries finished are kept, arnis-tiles
/// resumes from its chunk store, and anything cut off mid-write is removed.
#[allow(clippy::too_many_arguments)]
pub fn bake(
    exe: &Path,
    state: &Path,
    folder: &Path,
    bbox: &crate::coordinate_system::geographic::LLBBox,
    extracts: &[Extract],
    from: f64,
    threads: usize,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(f64, &str, &Transfer),
) -> Result<bool, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let published = Published::take(folder);
    let since = std::time::SystemTime::now();
    let mut cmd = command(exe, state, folder)?;
    cmd.args(["prepare", &format!("--bbox={}", bbox.arg()), "--threads"])
        .arg(threads.max(1).to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
    // Windows kills it with this process, however that ends; elsewhere the window's exit
    // stops the bake.
    if let Err(e) = crate::scale::child::adopt(&child) {
        let _ = child.kill();
        return Err(e);
    }
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // The download line is redrawn with \r, so stderr splits on both.
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        for b in std::io::BufReader::new(err).bytes().map_while(Result::ok) {
            if b == b'\r' || b == b'\n' {
                if !buf.is_empty()
                    && tx2
                        .send(String::from_utf8_lossy(&buf).into_owned())
                        .is_err()
                {
                    break;
                }
                buf.clear();
            } else {
                buf.push(b);
            }
        }
    });

    let start = Instant::now();
    let mut progress = BakeProgress::default();
    let mut rate = RateMeter::default();
    let mut rated = (usize::MAX, false);
    let mut tail: Vec<String> = Vec::new();
    let mut last_report = start.checked_sub(Duration::from_secs(1)).unwrap_or(start);
    loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            published.tidy(folder);
            return Ok(false);
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                progress.line(&line);
                if !line.trim().is_empty() {
                    tail.push(line);
                    if tail.len() > 8 {
                        tail.remove(0);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // Both pipes closed: the child is ending.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.wait().map_err(|e| e.to_string())?;
                if status.success() {
                    return Ok(true);
                }
                published.tidy(folder);
                let last = tail
                    .iter()
                    .rev()
                    .find(|l| l.starts_with("error:"))
                    .or(tail.last())
                    .map_or("no output", |l| l.trim());
                return Err(format!("arnis-tiles failed ({status}): {last}"));
            }
        }
        if last_report.elapsed() >= Duration::from_millis(500) {
            last_report = Instant::now();
            if let Some(e) = extracts.iter().find(|e| e.id == progress.id) {
                progress.observe(folder, e, since);
            }
            // A rate is a download's; each one starts its own.
            let downloading = matches!(progress.stage, Stage::Downloading(_));
            if rated != (progress.index, downloading) {
                rated = (progress.index, downloading);
                rate.reset();
            }
            let bps = if downloading {
                rate.add(start.elapsed().as_secs_f64(), progress.done_bytes)
            } else {
                0.0
            };
            let t = progress.transfer(extracts, from, bps, threads);
            let msg = format!("{} {}", progress.message(), clock(start.elapsed()));
            report(t.percent, &msg, &t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prepare_reads_dry_run_json() {
        // Stdout of `arnis-tiles prepare --dry-run --bbox 44.445,26.095,44.448,26.103`.
        let out = include_str!("../tests/fixtures/arnis_tiles_dry_run.txt");
        let p = parse_prepare(out).unwrap();
        assert_eq!(p.total_bytes, 329_538_806);
        assert_eq!(p.uncovered_points, 0);
        assert_eq!(
            p.extracts,
            vec![Extract {
                id: "romania".into(),
                name: "Romania".into(),
                bytes: 329_538_806,
                url: "https://download.geofabrik.de/europe/romania-latest.osm.pbf".into(),
            }]
        );
        // No extract covers open sea: the plan is empty, not missing.
        let sea = parse_prepare(
            "0 extract(s), 0.0 MB to download\n{\"bbox\":[0,0,1,1],\"extracts\":[],\"total_bytes\":0,\"uncovered_points\":1089}",
        )
        .unwrap();
        assert!(sea.extracts.is_empty());
        assert!(
            parse_prepare("fetching https://download.geofabrik.de/index-v1.json\n{oops").is_none()
        );
    }

    #[test]
    fn lookup_goes_next_to_the_exe_then_path() {
        let root = tempfile::tempdir().unwrap();
        let dir = |n: &str| {
            let d = root.path().join(n);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let (exe_dir, on_path) = (dir("exe"), dir("path"));
        let path = std::env::join_paths([dir("empty"), on_path.clone()]).unwrap();
        let find = || locate_in(Some(&exe_dir), Some(&path));

        assert_eq!(find(), None);
        std::fs::write(on_path.join(EXE), b"").unwrap();
        assert_eq!(find(), Some(on_path.join(EXE)));
        // The same folder named from the working directory is not looked in.
        #[cfg(unix)]
        {
            let cwd = std::env::current_dir().unwrap();
            let up: PathBuf = cwd.components().skip(1).map(|_| "..").collect();
            let rel = up.join(on_path.strip_prefix("/").unwrap());
            assert!(rel.is_relative() && rel.join(EXE).is_file());
            assert_eq!(locate_in(None, Some(rel.as_os_str())), None);
        }
        std::fs::write(exe_dir.join(EXE), b"").unwrap();
        assert_eq!(find(), Some(exe_dir.join(EXE)));
    }

    #[test]
    fn progress_follows_the_output_lines() {
        let pct = |p: &BakeProgress| p.transfer(&[], 0.0, 0.0, 1).percent;
        let mut p = BakeProgress::default();
        assert_eq!(pct(&p), 0.0);
        p.line("[1/2] liechtenstein (3 MB)");
        assert_eq!(p.message(), "Downloading liechtenstein (1/2): 0%");
        p.line("    downloaded 2 MB (50%)    ");
        assert_eq!(pct(&p), 10.0);
        p.line("    downloaded 3 MB          ");
        assert_eq!(
            (pct(&p), p.message().as_str()),
            (20.0, "Baking liechtenstein (1/2)...")
        );
        p.line("    120 ways, 30 pois, 4 relations -> 90 tiles, 1 MB in 2s");
        assert_eq!(pct(&p), 42.5);
        p.line("[2/2] austria already baked, skipping");
        assert_eq!(pct(&p), 92.5);
        assert_eq!(p.message(), "Writing the austria archive (2/2)...");
        p.line("archive ready: arnis --osm-tiles-url out ...");
        assert_eq!(pct(&p), 92.5, "an unknown line changes nothing");
    }

    fn extract(id: &str, bytes: u64) -> Extract {
        Extract {
            id: id.into(),
            name: id.into(),
            bytes,
            url: format!("https://download.geofabrik.de/europe/{id}-latest.osm.pbf"),
        }
    }

    #[test]
    fn the_files_in_the_folder_carry_the_bytes_between_output_lines() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let pbf = folder.join("work").join("pbf");
        std::fs::create_dir_all(&pbf).unwrap();
        let since = std::time::SystemTime::now() - Duration::from_secs(5);
        let li = extract("liechtenstein", 4_000);
        let all = [extract("austria", 10), li.clone()];
        let mut p = BakeProgress::default();
        p.line("[2/2] liechtenstein (0 MB)");
        // A quiet download (fetched while the last extract baked) grows its .part.
        std::fs::write(
            pbf.join("liechtenstein-latest.osm.pbf.part"),
            vec![0u8; 1_000],
        )
        .unwrap();
        p.observe(folder, &li, since);
        let t = p.transfer(&all, 0.0, 500.0, 6);
        assert_eq!(
            (t.stage, t.item, t.items, t.done_bytes, t.total_bytes),
            (crate::transfer::Stage::Download, 2, 2, 1_000, 4_000)
        );
        assert_eq!(t.percent, 55.0, "item 2 of 2, a quarter downloaded");
        assert_eq!(
            (t.threads, t.rate_bps, t.name.as_str()),
            (6, 500.0, "liechtenstein")
        );
        // Renamed: downloaded, so it bakes, and the chunk store grows.
        std::fs::rename(
            pbf.join("liechtenstein-latest.osm.pbf.part"),
            pbf.join("liechtenstein-latest.osm.pbf"),
        )
        .unwrap();
        p.observe(folder, &li, since);
        std::fs::write(
            folder.join("work").join("chunks-liechtenstein.db"),
            vec![0u8; 1_800],
        )
        .unwrap();
        p.observe(folder, &li, since);
        let t = p.transfer(&all, 0.0, 0.0, 6);
        assert_eq!(
            (t.stage, t.done_bytes, t.total_bytes),
            (crate::transfer::Stage::Bake, 1_800, 0)
        );
        // Half the typical store of a 4 kB extract.
        assert!(
            (t.percent - (50.0 + 0.5 * (40.0 + 45.0 * 0.5))).abs() < 1e-9,
            "{}",
            t.percent
        );
        // Writing: the dated archive being written.
        p.line("    42108 ways, 18842 pois, 244 relations -> 107 tiles, 3 MB in 0s");
        std::fs::write(
            folder.join("liechtenstein-20261006.pmtiles"),
            vec![0u8; 700],
        )
        .unwrap();
        p.observe(folder, &li, since);
        let t = p.transfer(&all, 0.0, 0.0, 6);
        assert_eq!(
            (t.stage, t.done_bytes),
            (crate::transfer::Stage::Finalize, 700)
        );
        // Another extract's files say nothing about this one.
        p.observe(folder, &all[0], since);
        assert_eq!(p.transfer(&all, 0.0, 0.0, 6).done_bytes, 700);
    }

    #[test]
    fn a_stopped_bake_leaves_only_whole_listed_archives() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let index = r#"{"archives":[{"file":"austria-1.pmtiles"}]}"#;
        std::fs::write(folder.join(INDEX), index).unwrap();
        std::fs::write(folder.join("austria-1.pmtiles"), b"whole").unwrap();
        // Not listed, but there before the bake: not this bake's to remove.
        std::fs::write(folder.join("older-0.pmtiles"), b"kept").unwrap();
        let before = Published::take(folder);

        // Stopped while writing the next archive and the index.
        std::fs::write(folder.join("switzerland-1.pmtiles"), b"half").unwrap();
        std::fs::write(folder.join(INDEX), r#"{"archives":[{"file":"aus"#).unwrap();
        let pbf = folder.join("work").join("pbf");
        std::fs::create_dir_all(&pbf).unwrap();
        std::fs::write(pbf.join("switzerland-latest.osm.pbf.part"), b"half").unwrap();
        std::fs::write(folder.join("work").join("chunks-switzerland.db"), b"resume").unwrap();
        assert_eq!(before.tidy(folder), 2);
        assert_eq!(std::fs::read_to_string(folder.join(INDEX)).unwrap(), index);
        assert!(folder.join("austria-1.pmtiles").is_file());
        assert!(folder.join("older-0.pmtiles").is_file());
        assert!(!folder.join("switzerland-1.pmtiles").exists());
        assert!(!pbf.join("switzerland-latest.osm.pbf.part").exists());
        assert!(folder.join("work").join("chunks-switzerland.db").is_file());

        // A country published before the stop stays, with its index entry.
        let before = Published::take(folder);
        std::fs::write(folder.join("liechtenstein-1.pmtiles"), b"whole").unwrap();
        let both =
            r#"{"archives":[{"file":"austria-1.pmtiles"},{"file":"liechtenstein-1.pmtiles"}]}"#;
        std::fs::write(folder.join(INDEX), both).unwrap();
        assert_eq!(before.tidy(folder), 0);
        assert!(folder.join("liechtenstein-1.pmtiles").is_file());
    }
}
