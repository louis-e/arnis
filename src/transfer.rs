//! Download and bake progress past a bare percentage: the stage, the bytes
//! moved, the rate and the threads, for the bar in the OSM Data Source panel.
//!
//! It rides on the existing channels: the `progress-update` event carries it
//! as a `transfer` field, and a child process (`--progress json`) sends it as
//! a `transfer` record, which the parent passes on to the window.

use std::collections::VecDeque;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    #[default]
    Download,
    Bake,
    Finalize,
}

/// Where one download-and-bake job is.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Transfer {
    pub stage: Stage,
    /// The extract being worked on (`liechtenstein`).
    pub name: String,
    /// 1-based, of `items`.
    pub item: usize,
    pub items: usize,
    pub done_bytes: u64,
    /// 0 when unknown.
    pub total_bytes: u64,
    /// Bytes a second over the last few seconds; 0 when unknown.
    pub rate_bps: f64,
    /// Threads the bake runs on.
    pub threads: usize,
    /// `threads` as a share of the cores.
    pub cpu_pct: u32,
    /// Downloads allowed at once.
    pub downloads: u32,
    /// The whole job, 0-100.
    pub percent: f64,
}

/// Where a stage ends within one item, in percent: the download is the first
/// 40 %, the bake runs to 85 % and finishing the output takes the rest.
const DOWNLOAD_END: f64 = 40.0;
const BAKE_END: f64 = 85.0;

/// Percent done of a job of `items` items when item `index` (0-based) is
/// `frac` (0-1) of the way through `stage`.
pub fn job_percent(index: usize, items: usize, stage: Stage, frac: f64) -> f64 {
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let within = match stage {
        Stage::Download => DOWNLOAD_END * frac,
        Stage::Bake => DOWNLOAD_END + (BAKE_END - DOWNLOAD_END) * frac,
        Stage::Finalize => BAKE_END + (100.0 - BAKE_END) * frac,
    };
    ((index as f64 * 100.0 + within) / items.max(1) as f64).min(100.0)
}

/// Logical cores of this machine.
pub fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// `threads` as a whole-percent share of `cores`, at most 100.
pub fn cpu_pct(threads: usize, cores: usize) -> u32 {
    let cores = cores.max(1);
    ((threads * 100 + cores / 2) / cores).min(100) as u32
}

/// Seconds of samples a rate is taken over.
const RATE_WINDOW_S: f64 = 8.0;

/// Bytes a second, from `(seconds, bytes so far)` samples over the last few
/// seconds. A count that goes down starts over: the next item began.
#[derive(Debug, Default)]
pub struct RateMeter {
    samples: VecDeque<(f64, u64)>,
}

impl RateMeter {
    pub fn reset(&mut self) {
        self.samples.clear();
    }

    /// Adds a sample at `t` seconds and returns the rate; 0 until two samples
    /// at least a quarter second apart exist.
    pub fn add(&mut self, t: f64, bytes: u64) -> f64 {
        if self.samples.back().is_some_and(|&(_, b)| bytes < b) {
            self.samples.clear();
        }
        self.samples.push_back((t, bytes));
        while self.samples.len() > 2 && self.samples[1].0 <= t - RATE_WINDOW_S {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        let dt = t - t0;
        if dt < 0.25 {
            return 0.0;
        }
        (bytes - b0) as f64 / dt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_moves_through_its_items_and_stages_in_order() {
        assert_eq!(job_percent(0, 1, Stage::Download, 0.0), 0.0);
        assert_eq!(job_percent(0, 1, Stage::Download, 0.5), 20.0);
        assert_eq!(job_percent(0, 1, Stage::Bake, 0.0), 40.0);
        assert_eq!(job_percent(0, 1, Stage::Bake, 1.0), 85.0);
        assert_eq!(job_percent(0, 1, Stage::Finalize, 1.0), 100.0);
        // The second of two items starts at half.
        assert_eq!(job_percent(1, 2, Stage::Download, 0.0), 50.0);
        assert_eq!(job_percent(1, 2, Stage::Finalize, 1.0), 100.0);
        // Out-of-range fractions and an empty job stay on the bar.
        assert_eq!(job_percent(0, 1, Stage::Bake, 7.0), 85.0);
        assert_eq!(job_percent(0, 1, Stage::Download, f64::NAN), 0.0);
        assert_eq!(job_percent(0, 0, Stage::Finalize, 1.0), 100.0);
        let mut last = 0.0;
        for item in 0..3 {
            for stage in [Stage::Download, Stage::Bake, Stage::Finalize] {
                for f in [0.0, 0.5, 1.0] {
                    let p = job_percent(item, 3, stage, f);
                    assert!(p >= last, "{item} {stage:?} {f}: {p} < {last}");
                    last = p;
                }
            }
        }
    }

    #[test]
    fn the_cpu_share_rounds_and_caps() {
        assert_eq!(cpu_pct(12, 16), 75);
        assert_eq!(cpu_pct(21, 24), 88);
        assert_eq!(cpu_pct(1, 3), 33);
        assert_eq!(cpu_pct(64, 16), 100);
        assert_eq!(cpu_pct(4, 0), 100);
    }

    #[test]
    fn the_rate_follows_the_last_seconds() {
        let mut r = RateMeter::default();
        assert_eq!(r.add(0.0, 0), 0.0);
        assert_eq!(r.add(0.1, 1_000), 0.0, "too soon to tell");
        assert_eq!(r.add(1.0, 10_000_000), 10_000_000.0);
        assert_eq!(r.add(2.0, 20_000_000), 10_000_000.0);
        // A slower stretch, once the fast start has left the window.
        for s in 3u32..=12 {
            r.add(f64::from(s), 20_000_000 + (u64::from(s) - 2) * 1_000_000);
        }
        let slow = r.add(13.0, 31_000_000);
        assert!((slow - 1_000_000.0).abs() < 1.0, "{slow}");
        // A smaller count is the next item: start over.
        assert_eq!(r.add(14.0, 5), 0.0);
        assert_eq!(r.add(15.0, 2_000_005), 2_000_000.0);
    }
}
