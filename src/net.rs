//! Process-wide ceiling on in-flight HTTP requests.

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

#[cfg(test)]
mod tests {
    use super::*;

    // The counter is process-global, so these cases cannot observe each other.
    static SERIALIZE: Mutex<()> = Mutex::new(());

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
