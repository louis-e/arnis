//! Process-wide ceiling on in-flight HTTP requests, and how a request failed.

use std::sync::{Condvar, Mutex};

/// Sits above every per-provider download pool (4-8 threads), so ordinary
/// fetching never waits on it. It only bounds unrelated fan-outs stacking on
/// top of each other, which is what exhausts the Windows I/O resource limits
/// the tokio driver panics on.
const MAX_CONCURRENT_REQUESTS: usize = 16;

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
    while *in_flight >= MAX_CONCURRENT_REQUESTS {
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
