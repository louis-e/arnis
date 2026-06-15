use std::time::{Duration, Instant};

/// Lightweight per-phase wall-clock reporter, active only under `--benchmark`.
/// Prints `[BENCHMARK] <label>_ms=<n>` lines to stderr.
pub struct Bench {
    on: bool,
    last: Instant,
}

impl Bench {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            last: Instant::now(),
        }
    }

    /// Report time since the previous mark/reset, then restart the clock.
    pub fn mark(&mut self, label: &str) {
        if self.on {
            eprintln!("[BENCHMARK] {label}_ms={}", self.last.elapsed().as_millis());
            self.last = Instant::now();
        }
    }

    /// Restart the clock without printing.
    pub fn reset(&mut self) {
        self.last = Instant::now();
    }

    /// Report a pre-accumulated duration (for phases timed across a loop).
    pub fn report(&self, label: &str, dur: Duration) {
        if self.on {
            eprintln!("[BENCHMARK] {label}_ms={}", dur.as_millis());
        }
    }
}
