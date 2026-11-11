//! `--progress json`: generation progress as NDJSON on stdout, for programs
//! driving the CLI. One record per line, each tagged with the protocol version:
//!
//! ```text
//! {"v":1,"type":"phase","name":"Generating area...","progress":20.0}
//! {"v":1,"type":"progress","progress":35.5}
//! {"v":1,"type":"error","message":"..."}
//! {"v":1,"type":"done","chunks":880,"cpu_s":388.0,"peak_rss_mb":2310,"wall_s":41.2}
//! ```
//!
//! `phase` marks a new status message (`progress` is null for one that leaves
//! the bar alone), `progress` a new percentage (monotonic, two decimals).
//! `done` closes a successful run: wall and CPU seconds, peak resident memory
//! and the Java/Bedrock chunks written. The usual human-readable lines still
//! go to stdout around these, so a reader keeps the lines starting `{"v":`.
//!
//! `piece` records (state, rect, bounds, pieces_done) and others go through `record`.
//!
//! Records come from the same emit points that drive the GUI progress bar, so
//! nothing here is called unless `enable` was.

use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);

// Highest percentage sent so far, in hundredths, like the GUI's floor.
static FLOOR: AtomicU32 = AtomicU32::new(0);

static LAST_PHASE: Mutex<String> = Mutex::new(String::new());

/// Chunks the Java and Bedrock writers have written, for the `done` record.
pub static CHUNKS_WRITTEN: AtomicU64 = AtomicU64::new(0);

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `v` and `type` lead every line so a reader can pick records out by prefix;
/// serde_json sorts a plain object's keys, so they are fields of their own.
#[derive(Serialize)]
struct Record<'a> {
    v: u8,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(flatten)]
    body: Value,
}

fn line(kind: &str, body: Value) -> Option<String> {
    let record = Record { v: 1, kind, body };
    serde_json::to_string(&record).ok()
}

fn emit(kind: &str, body: Value) {
    if let Some(line) = line(kind, body) {
        println!("{line}");
    }
}

/// Mirrors one GUI progress emit. Error messages are left to `error`, which
/// gets them untruncated.
pub fn progress(progress: f64, message: &str) {
    if !enabled() || message.starts_with("Error!") {
        return;
    }
    // Only a percentage above the floor is news; the stages report out of order.
    let pct = (progress >= 0.0).then_some((progress * 100.0) as u32);
    let advanced = pct.is_some_and(|p| FLOOR.fetch_max(p, Ordering::Relaxed) < p);
    let shown = f64::from(FLOOR.load(Ordering::Relaxed)) / 100.0;

    let mut last = LAST_PHASE.lock().unwrap_or_else(|e| e.into_inner());
    if !message.is_empty() && *last != message {
        message.clone_into(&mut last);
        emit(
            "phase",
            json!({"name": message, "progress": pct.map(|_| shown)}),
        );
    } else if advanced {
        emit("progress", json!({ "progress": shown }));
    }
}

/// Any other record type, such as a job's `piece` and `result` records.
pub fn record(kind: &str, body: Value) {
    if enabled() {
        emit(kind, body);
    }
}

pub fn error(message: &str) {
    if enabled() {
        emit("error", json!({ "message": message }));
    }
}

fn done_body(started: Instant) -> Value {
    let usage = process_usage();
    json!({
        "wall_s": (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        "cpu_s": usage.map(|(cpu_s, _)| (cpu_s * 1000.0).round() / 1000.0),
        "peak_rss_mb": usage.map(|(_, peak)| peak),
        "chunks": CHUNKS_WRITTEN.load(Ordering::Relaxed),
    })
}

/// The closing record of a successful run started at `started`.
pub fn done(started: Instant) {
    record("done", done_body(started));
}

/// CPU seconds (user + kernel) and peak resident memory in MB of this
/// process. sysinfo only reports current usage, so this asks the OS.
#[cfg(windows)]
fn process_usage() -> Option<(f64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    // FILETIME counts 100 ns ticks.
    let secs = |t: FILETIME| {
        ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64 / 1e7
    };
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    let mut memory = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: the pseudo-handle of the current process needs no closing, and
    // every out-pointer is a live local of the type the call expects.
    unsafe {
        let process = GetCurrentProcess();
        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user).ok()?;
        if !K32GetProcessMemoryInfo(process, &mut memory, memory.cb).as_bool() {
            return None;
        }
    }
    let peak_mb = (memory.PeakWorkingSetSize / (1024 * 1024)) as u64;
    Some((secs(kernel) + secs(user), peak_mb))
}

#[cfg(unix)]
fn process_usage() -> Option<(f64, u64)> {
    // SAFETY: getrusage only writes the struct it is handed.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        usage
    };
    let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    // ru_maxrss is kilobytes on Linux and bytes on macOS.
    let peak_bytes = if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64
    } else {
        usage.ru_maxrss as u64 * 1024
    };
    Some((
        secs(usage.ru_utime) + secs(usage.ru_stime),
        peak_bytes / (1024 * 1024),
    ))
}

#[cfg(not(any(windows, unix)))]
fn process_usage() -> Option<(f64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_reports_cpu_and_memory() {
        let (cpu_s, peak_mb) = process_usage().expect("usage");
        assert!(cpu_s >= 0.0);
        assert!(peak_mb > 0);
    }

    #[test]
    fn done_record_has_contract_fields() {
        let line = line("done", done_body(std::time::Instant::now())).unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "done");
        assert_eq!(v["v"], 1);
        assert!(line.starts_with(r#"{"v":1,"type":"done""#), "{line}");
        for k in ["wall_s", "cpu_s", "peak_rss_mb", "chunks"] {
            assert!(v[k].is_number(), "{k} missing or not numeric: {line}");
        }
    }
}
