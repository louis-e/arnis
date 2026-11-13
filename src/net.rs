//! Process-wide ceiling on in-flight HTTP requests, and how a request failed.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// Sits above every per-provider download pool (4-8 threads), so ordinary
/// fetching never waits on it. It only bounds unrelated fan-outs stacking on
/// top of each other, which is what exhausts the Windows I/O resource limits
/// the tokio driver panics on.
pub const MAX_CONCURRENT_REQUESTS: usize = 16;

/// The ceiling in force: `MAX_CONCURRENT_REQUESTS` unless `--max-downloads` set another.
static MAX_REQUESTS: AtomicUsize = AtomicUsize::new(MAX_CONCURRENT_REQUESTS);

/// Changes the ceiling. Call before any download starts.
pub fn set_max_requests(n: usize) {
    MAX_REQUESTS.store(n.max(1), Ordering::Relaxed);
}

pub fn max_requests() -> usize {
    MAX_REQUESTS.load(Ordering::Relaxed)
}

/// `--offline`: every download is refused before it starts.
static OFFLINE: AtomicBool = AtomicBool::new(false);
/// What an offline run wanted and the cache did not have, with a count of the refused requests.
static OFFLINE_MISSES: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

/// Turning it on forgets the misses of an earlier run in this process.
pub fn set_offline(on: bool) {
    if on {
        OFFLINE_MISSES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    OFFLINE.store(on, Ordering::Relaxed);
}

/// `--prewarm`: the run is there to fill the caches.
static PREWARM: AtomicBool = AtomicBool::new(false);

/// Turning it on forgets what an earlier run in this process could not cache.
pub fn set_prewarm(on: bool) {
    if on {
        UNCACHED.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    PREWARM.store(on, Ordering::Relaxed);
}

pub fn prewarm() -> bool {
    PREWARM.load(Ordering::Relaxed)
}

pub fn offline() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

/// The window's saved Offline Mode, which holds between runs too.
static OFFLINE_MODE: AtomicBool = AtomicBool::new(false);

#[cfg_attr(not(any(feature = "gui", test)), allow(dead_code))]
pub fn set_offline_mode(on: bool) {
    OFFLINE_MODE.store(on, Ordering::Relaxed);
}

/// Whether what is not a run's data stays in as well: telemetry and the update check.
pub fn offline_mode() -> bool {
    offline() || OFFLINE_MODE.load(Ordering::Relaxed)
}

/// A window run's `--offline` and `--prewarm`, which end with it: both are process-wide, and
/// the previews between two runs read them too.
#[cfg_attr(not(any(feature = "gui", test)), allow(dead_code))]
pub struct RunScope(());

#[cfg_attr(not(any(feature = "gui", test)), allow(dead_code))]
pub fn begin_run(offline: bool, prewarm: bool) -> RunScope {
    set_prewarm(prewarm);
    OFFLINE.store(offline, Ordering::SeqCst);
    // A preview caught mid-fetch is refused from here on; the misses are cleared after it.
    while offline && PREVIEWS.load(Ordering::SeqCst) > 0 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    set_offline(offline);
    RunScope(())
}

impl Drop for RunScope {
    fn drop(&mut self) {
        set_offline(false);
        set_prewarm(false);
    }
}

/// Window previews that are fetching. An offline run waits for them, so what they are
/// refused never counts as its misses.
static PREVIEWS: AtomicUsize = AtomicUsize::new(0);

/// Held by a window preview while it fetches.
#[cfg_attr(not(any(feature = "gui", test)), allow(dead_code))]
pub struct Preview(());

/// `Err` during an offline run: turned away here, a preview is refused no download.
#[cfg_attr(not(any(feature = "gui", test)), allow(dead_code))]
pub fn preview() -> Result<Preview, String> {
    PREVIEWS.fetch_add(1, Ordering::SeqCst);
    let preview = Preview(());
    if OFFLINE.load(Ordering::SeqCst) {
        return Err("Offline Mode: no preview while the run is going.".to_string());
    }
    Ok(preview)
}

impl Drop for Preview {
    fn drop(&mut self) {
        PREVIEWS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Called in front of a download that only a cache miss reaches. Offline it refuses, and
/// records `what` so the run can stop and name everything the cache lacks instead of building
/// flat ground or leaving objects out.
pub fn ensure_online(what: &str) -> Result<(), String> {
    if !offline() {
        return Ok(());
    }
    *OFFLINE_MISSES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(what.to_string())
        .or_default() += 1;
    Err(format!("--offline: {what} is not in the cache"))
}

/// Everything [`ensure_online`] refused so far, by name.
pub fn offline_misses() -> Vec<(String, usize)> {
    let misses = OFFLINE_MISSES.lock().unwrap_or_else(|e| e.into_inner());
    misses.iter().map(|(k, v)| (k.clone(), *v)).collect()
}

/// What a `--prewarm` run fetched and could not cache, or could not fetch at all.
static UNCACHED: Mutex<std::collections::BTreeSet<String>> =
    Mutex::new(std::collections::BTreeSet::new());

/// Records that `what` did not reach the cache, so a prewarm does not claim it did.
pub fn note_uncached(what: &str) {
    if prewarm() {
        UNCACHED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(what.to_string());
    }
}

pub fn uncached() -> Vec<String> {
    let uncached = UNCACHED.lock().unwrap_or_else(|e| e.into_inner());
    uncached.iter().cloned().collect()
}

static IN_FLIGHT: Mutex<usize> = Mutex::new(0);
static SLOT_FREED: Condvar = Condvar::new();

/// Releases its slot on drop, including while unwinding from a panic.
pub struct RequestPermit {
    _private: (),
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        {
            let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
            *in_flight = in_flight.saturating_sub(1);
        }
        SLOT_FREED.notify_one();
    }
}

/// The in-flight counter is process wide, so a test that asserts an exact
/// value of it cannot run beside a test that takes a permit of its own. Every
/// test in the crate that does either holds this first; the tests below and the
/// Mapillary fetch tests are the current users.
#[cfg(test)]
pub static PERMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Blocks until a slot is free. Hold it across a single request/response only:
/// acquiring a second permit while holding one would deadlock.
#[must_use]
pub fn request_permit() -> RequestPermit {
    let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    while *in_flight >= max_requests() {
        in_flight = SLOT_FREED
            .wait(in_flight)
            .unwrap_or_else(|e| e.into_inner());
    }
    *in_flight += 1;
    RequestPermit { _private: () }
}

/// How a request failed, coarse enough to report: never a URL, host or address.
pub fn failure_kind(e: &reqwest::Error) -> String {
    if let Some(status) = e.status() {
        return format!("http_{}", status.as_u16());
    }
    let causes = std::iter::successors(std::error::Error::source(e), |s| s.source())
        .map(|s| s.to_string().to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" | ");
    // A dropped connection first: rustls words it with "tls" too.
    let dropped = ["reset", "forcibly closed", "close_notify", "unexpected eof"];
    let kind = if e.is_timeout() {
        "timeout"
    } else if causes.contains("dns error") {
        "dns"
    } else if dropped.iter().any(|d| causes.contains(d)) {
        "reset"
    } else if causes.contains("certificate") || causes.contains("tls") {
        "tls"
    } else if e.is_connect() {
        "connect"
    } else if e.is_body() || e.is_decode() {
        "body"
    } else {
        "other"
    };
    kind.to_string()
}

/// Whether `tag` is something [`failure_kind`] returns.
#[cfg_attr(not(any(feature = "gui", test)), allow(dead_code))]
pub fn is_failure_kind(tag: &str) -> bool {
    const KINDS: [&str; 7] = ["timeout", "dns", "tls", "reset", "connect", "body", "other"];
    KINDS.contains(&tag)
        || tag
            .strip_prefix("http_")
            .is_some_and(|code| code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The counter is process-global, so these cases cannot observe each other
    // or anything else in the crate that takes a permit.
    use super::PERMIT_TEST_LOCK as SERIALIZE;

    fn in_flight() -> usize {
        *IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn offline_refuses_and_names_what_was_missing() {
        // Downloads elsewhere in the crate would be refused while this runs.
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let _floor = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let what = "net test source";
        assert!(ensure_online(what).is_ok());
        set_offline(true);
        let refused = (ensure_online(what), ensure_online(what));
        set_offline(false);
        assert!(refused.0.unwrap_err().contains(what));
        assert!(refused.1.is_err());
        assert!(offline_misses().contains(&(what.to_string(), 2)));
    }

    #[test]
    fn offline_mode_is_the_saved_setting_or_an_offline_run() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let _floor = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert!(!offline_mode());
        set_offline_mode(true);
        let saved = (offline_mode(), offline());
        set_offline_mode(false);
        // The setting alone refuses no download: that is the run's `--offline`.
        assert_eq!(saved, (true, false));
        let run = begin_run(true, false);
        assert!(offline_mode());
        drop(run);
        assert!(!offline_mode());
    }

    #[test]
    fn a_run_takes_its_offline_and_prewarm_with_it() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let _floor = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let run = begin_run(true, false);
        assert!(offline() && !prewarm());
        // Turned away up front, so the run's misses stay its own.
        assert!(preview().is_err());
        assert!(offline_misses().is_empty());
        drop(run);
        assert!(!offline());
        assert!(preview().is_ok());

        let run = begin_run(false, true);
        assert!(prewarm() && !offline());
        note_uncached("net test run");
        drop(run);
        assert!(!prewarm());
        // The next prewarm starts with nothing held against it.
        drop(begin_run(false, true));
        assert!(uncached().is_empty());
    }

    #[test]
    fn an_offline_run_waits_for_a_preview_and_drops_its_misses() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let _floor = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let what = "net test preview";
        let fetching = preview().unwrap();
        let preview = std::thread::spawn(move || {
            while ensure_online(what).is_ok() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            drop(fetching);
        });
        let run = begin_run(true, false);
        preview.join().unwrap();
        assert!(!offline_misses().iter().any(|(w, _)| w == what));
        drop(run);
    }

    #[test]
    fn only_a_prewarm_keeps_what_it_could_not_cache() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        note_uncached("net test online");
        set_prewarm(true);
        note_uncached("net test prewarm");
        note_uncached("net test prewarm");
        set_prewarm(false);
        let got = uncached();
        assert!(!got.iter().any(|w| w == "net test online"));
        assert_eq!(got.iter().filter(|w| *w == "net test prewarm").count(), 1);
    }

    #[test]
    fn permit_releases_on_drop() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        {
            let _permit = request_permit();
            assert_eq!(in_flight(), 1);
        }
        assert_eq!(in_flight(), 0);
    }

    #[test]
    fn permit_releases_while_unwinding() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let result = std::panic::catch_unwind(|| {
            let _permit = request_permit();
            panic!("boom");
        });
        assert!(result.is_err());
        assert_eq!(in_flight(), 0);
    }

    #[test]
    fn concurrent_holders_never_exceed_the_cap() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let threads: Vec<_> = (0..MAX_CONCURRENT_REQUESTS * 2)
            .map(|_| {
                std::thread::spawn(|| {
                    let _permit = request_permit();
                    assert!(in_flight() <= MAX_CONCURRENT_REQUESTS);
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(in_flight(), 0);
    }
}
