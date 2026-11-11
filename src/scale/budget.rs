//! How many pieces of a job run at once, and what each may use.
//!
//! A fixed count (`--one-world-workers N`) splits what one run would take:
//! `--threads` else 90% of the cores, `--ram-budget-mb` else
//! free memory, `--max-downloads` else 16.
//!
//! `auto` sizes from the machine:
//! - budget = cores x 90%, or `--threads`, as a fixed count takes;
//! - W = clamp(min(pieces, budget / MIN_THREADS, free RAM / piece RAM), 2, MAX_WORKERS);
//! - T = budget / W.
//!
//! A few fat workers beat many thin ones (T3: 4 x 5 threads was 1.26-1.53x one
//! process; 16 x 1 was slower; B3 on 24 threads: 4 x 5 beat 6 x 3 and 2 x 10), hence
//! the thread floor of 5 and the cap of 6. Piece
//! RAM starts from `estimate_piece_mb` and, once pieces finish, follows the
//! largest peak they reported (`Sizing::cap_for`).

use crate::args::{Args, UnitSize, Workers};
use std::ffi::OsString;

/// Threads below which a piece is not worth its own process.
const MIN_THREADS: usize = 5;
const MIN_WORKERS: usize = 2;
pub const MAX_WORKERS: usize = 6;
/// CPU share `auto` plans for, leaving the machine usable.
const AUTO_CPU_TARGET: usize = 90;

/// Peak memory of one piece of `regions` regions before any has reported.
/// From B3 (S16, warm caches, 5 threads a piece): 8 x 8 pieces peaked at 825 MB
/// a worker, 4 x 4 at 529; a whole 64-region run at scale 0.05 at 0.9 GB (T3).
/// More threads a piece raise it (8 x 8 with 10 threads: 1.3 GB), which
/// `Sizing::cap_for` catches once a piece reports.
pub fn estimate_piece_mb(regions: u64, scale: f64) -> u64 {
    let per_region = if scale >= 0.5 { 9 } else { 8 };
    400 + per_region * regions
}

/// Largest side, in regions, that `--unit-regions auto` gives a piece: 8, 4 or 2, the largest
/// of which two pieces fit in `--ram-budget-mb`, else in the machine's memory less the
/// reserve. Total memory rather than free, so a resumed job cuts the same pieces.
pub fn auto_max_regions(args: &Args) -> i32 {
    let budget = args.ram_budget_mb.unwrap_or_else(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let total = sys.total_memory() / (1024 * 1024);
        total.saturating_sub(reserve_mb(total))
    });
    [8, 4]
        .into_iter()
        .find(|&s| estimate_piece_mb(s * s, args.scale) * MIN_WORKERS as u64 <= budget)
        .map_or(2, |s| s as i32)
}

/// Memory kept free for the system and everything else running.
fn reserve_mb(total_mb: u64) -> u64 {
    (total_mb / 10).max(2048)
}

/// Ground at scale 1, km², below which `auto` builds one plain run instead of pieces. B3
/// (Bucharest, warm caches, 24 threads), median wall of one run vs 8 x 8 pieces x 4:
/// 100 km² 23.4 vs 28.5 s, 150 km² 35.6 vs 36.5 s, 200 km² 45.7 vs 42.6 s (pieces first
/// 3% faster), 256 km² 72.7 vs 64.9 s; set a bit above 200.
pub const AUTO_SINGLE_BELOW_KM2: f64 = 220.0;

/// Whether `single_below_break_even` builds a selection of `km2` (ground at scale 1 times
/// scale squared) in one plain run; the window's plan asks the same.
pub fn builds_single(args: &Args, km2: f64) -> bool {
    let u = &args.units;
    u.unit_regions == Some(UnitSize::Auto)
        && u.one_world_workers == Some(Workers::Auto)
        && u.one_world_unit.is_none()
        && u.only_pieces.is_none()
        && u.plan_mask.is_none()
        && u.snap_regions.is_none()
        && u.margin_regions == 0
        && u.piece_order.is_none()
        && !u.skip_built
        && !u.stitch_map
        && !args.prewarm
        && !args.prewarm_first
        && km2 < AUTO_SINGLE_BELOW_KM2
}

/// `--unit-regions auto --one-world-workers auto` on a selection smaller than
/// `AUTO_SINGLE_BELOW_KM2`, with no other job flag: drops both, so the One World is
/// built in one plain run, which is faster there.
pub fn single_below_break_even(args: &mut Args) {
    let km2 = args
        .bbox
        .map_or(f64::INFINITY, |b| b.area_km2() * args.scale * args.scale);
    if builds_single(args, km2) {
        println!(
            "One World: {km2:.0} km² is below {AUTO_SINGLE_BELOW_KM2:.0} km², built in one run"
        );
        args.units.unit_regions = None;
        args.units.one_world_workers = None;
    }
}

/// W and T for `auto`. Never fewer than two workers: a job worth cutting in
/// pieces is worth overlapping them. `sizing` never starts more than there
/// are pieces.
pub fn auto_plan(
    budget_threads: usize,
    pieces: usize,
    usable_mb: u64,
    piece_mb: u64,
) -> (usize, usize) {
    let by_cpu = budget_threads / MIN_THREADS;
    let by_ram = (usable_mb / piece_mb.max(1)) as usize;
    let workers = pieces
        .min(by_cpu)
        .min(by_ram)
        .clamp(MIN_WORKERS, MAX_WORKERS);
    (workers, (budget_threads / workers).max(1))
}

#[derive(Debug, PartialEq)]
pub struct Sizing {
    pub workers: usize,
    pub threads: usize,
    pub ram_mb: Option<u64>,
    pub downloads: u32,
    /// Memory the running pieces may share, when `auto` watches it.
    pub usable_mb: Option<u64>,
}

impl Sizing {
    pub fn child_args(&self, mut argv: Vec<OsString>) -> Vec<OsString> {
        argv.extend([
            "--threads".into(),
            self.threads.to_string().into(),
            "--max-downloads".into(),
            self.downloads.to_string().into(),
        ]);
        if let Some(mb) = self.ram_mb {
            argv.extend(["--ram-budget-mb".into(), mb.to_string().into()]);
        }
        argv
    }

    pub fn child_env(&self) -> [(&'static str, String); 2] {
        [
            ("RAYON_NUM_THREADS", self.threads.to_string()),
            // The stock writer sizing, on this piece's share of the cores.
            (
                "ARNIS_FLUSH_THREADS",
                self.threads.div_ceil(4).clamp(1, 6).to_string(),
            ),
        ]
    }

    /// Pieces that may run together once a piece peaked at `peak_mb`.
    /// Only ever lowers the planned count.
    pub fn cap_for(&self, peak_mb: u64) -> usize {
        match self.usable_mb {
            Some(usable) => ((usable / peak_mb.max(1)) as usize).clamp(1, self.workers),
            None => self.workers,
        }
    }
}

/// Sizing for a job of `pieces` pieces, the largest `piece_regions` regions.
pub fn sizing(args: &Args, pieces: usize, piece_regions: u64) -> Sizing {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let pieces = pieces.max(1);
    let downloads = |w: usize| (args.max_downloads.unwrap_or(16) / w as u32).max(2);
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let free_mb = sys.available_memory() / (1024 * 1024);
    match args.units.one_world_workers {
        Some(Workers::Auto) => {
            let budget = args
                .thread_count()
                .unwrap_or(cores * AUTO_CPU_TARGET / 100)
                .max(1);
            let total = sys.total_memory() / (1024 * 1024);
            let free = args
                .ram_budget_mb
                .unwrap_or_else(|| free_mb.saturating_sub(reserve_mb(total)));
            let piece_mb = estimate_piece_mb(piece_regions, args.scale);
            let workers = auto_plan(budget, pieces, free, piece_mb).0.min(pieces);
            Sizing {
                workers,
                threads: (budget / workers).max(1),
                ram_mb: Some((free / workers as u64).max(1)),
                downloads: downloads(workers),
                usable_mb: Some(free),
            }
        }
        fixed => {
            let threads = args.thread_count().unwrap_or(cores * 9 / 10).max(1);
            let workers = match fixed {
                Some(Workers::N(n)) => n as usize,
                _ => 1,
            }
            .min(pieces);
            let ram_mb = args
                .ram_budget_mb
                .or_else(|| (workers > 1).then_some(free_mb));
            Sizing {
                workers,
                threads: (threads / workers).max(1),
                ram_mb: ram_mb.map(|mb| (mb / workers as u64).max(1)),
                downloads: downloads(workers),
                usable_mb: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_stays_between_2_and_6() {
        // 24 cores at 90%: 21 threads, 4 workers of 5+ threads.
        assert_eq!(auto_plan(21, 16, 30_000, 976), (4, 5));
        // Never more than six, however big the machine.
        assert_eq!(auto_plan(96, 100, 500_000, 720), (6, 16));
        // Short on memory, cores or pieces: still two.
        assert_eq!(auto_plan(18, 16, 100, 720).0, 2);
        assert_eq!(auto_plan(3, 16, 500_000, 720), (2, 1));
        assert_eq!(auto_plan(18, 1, 30_000, 720).0, 2);
    }

    #[test]
    fn piece_memory_follows_its_size_and_scale() {
        assert_eq!(estimate_piece_mb(16, 1.0), 544);
        assert!(estimate_piece_mb(64, 1.0) > estimate_piece_mb(64, 0.05));
        assert_eq!(reserve_mb(8_000), 2048);
        assert_eq!(reserve_mb(64_000), 6400);
    }

    #[test]
    fn a_measured_peak_only_ever_lowers_the_count() {
        let s = Sizing {
            workers: 4,
            threads: 4,
            ram_mb: Some(4000),
            downloads: 4,
            usable_mb: Some(16_000),
        };
        assert_eq!(s.cap_for(3000), 4);
        assert_eq!(s.cap_for(6000), 2);
        assert_eq!(s.cap_for(40_000), 1);
        let fixed = Sizing {
            usable_mb: None,
            ..s
        };
        assert_eq!(fixed.cap_for(40_000), 4);
    }

    fn parse(extra: &[&str]) -> Args {
        let mut cmd = vec![
            "arnis",
            "--output-dir",
            ".",
            "--bbox",
            "1,2,3,4",
            "--one-world",
        ];
        cmd.extend_from_slice(extra);
        <Args as clap::Parser>::parse_from(cmd)
    }

    #[test]
    fn auto_pieces_are_as_large_as_two_fit_the_memory() {
        // Two 8 x 8 pieces at scale 1 need 2 x 976 MB, two 4 x 4 2 x 544 MB.
        for (mb, side) in [
            (4000, 8),
            (1952, 8),
            (1951, 4),
            (1088, 4),
            (1087, 2),
            (10, 2),
        ] {
            let args = parse(&["--ram-budget-mb", &mb.to_string()]);
            assert_eq!(auto_max_regions(&args), side, "{mb} MB");
        }
    }

    #[test]
    fn a_fixed_count_splits_the_jobs_budget() {
        let s = sizing(
            &parse(&[
                "--one-world-workers",
                "4",
                "--threads",
                "20",
                "--ram-budget-mb",
                "8000",
            ]),
            16,
            16,
        );
        assert_eq!(
            s,
            Sizing {
                workers: 4,
                threads: 5,
                ram_mb: Some(2000),
                downloads: 4,
                usable_mb: None,
            }
        );
        // Never more workers than pieces, never less than a thread each.
        let s = sizing(
            &parse(&["--one-world-workers", "8", "--threads", "3"]),
            2,
            16,
        );
        assert_eq!((s.workers, s.threads), (2, 1));
        // One at a time: the job's own knobs, nothing invented.
        let s = sizing(&parse(&["--unit-regions", "2", "--threads", "6"]), 9, 4);
        assert_eq!((s.workers, s.threads, s.ram_mb), (1, 6, None));
    }

    #[test]
    fn auto_honours_an_explicit_thread_and_memory_budget() {
        let s = sizing(
            &parse(&[
                "--one-world-workers",
                "auto",
                "--threads",
                "16",
                "--ram-budget-mb",
                "6000",
            ]),
            16,
            16,
        );
        // 16 threads allow 3 workers, 6000 MB at ~544 MB a piece allow 11.
        assert_eq!((s.workers, s.threads, s.ram_mb), (3, 5, Some(2000)));
        assert_eq!(s.usable_mb, Some(6000));
    }

    // B3 (work/bench22/results.jsonl, S16 = 256 km2 of Bucharest, warm caches, 24 threads,
    // 31 GB): the lowest median was 8 x 8 regions with 4 workers, 64.9 s, peak 3,298 MB for
    // the whole job; 4 x 4 regions with 4 workers peaked at 2,116 MB.
    #[test]
    fn auto_plan_picks_measured_sweet_spot() {
        let total = 31 * 1024;
        let usable = total - reserve_mb(total);
        let side = auto_max_regions(&parse(&["--ram-budget-mb", &usable.to_string()]));
        assert_eq!(side, 8);
        let regions = (side * side) as u64;
        // S16 cuts into 16 pieces of up to 8 x 8.
        let plan = auto_plan(
            24 * AUTO_CPU_TARGET / 100,
            16,
            usable,
            estimate_piece_mb(regions, 1.0),
        );
        assert_eq!(plan, (4, 5));
    }

    #[test]
    fn estimate_tracks_measured_peak() {
        for (regions, job_peak_mb, workers) in [(64, 3298.0, 4.0), (16, 2116.0, 4.0)] {
            let peak = job_peak_mb / workers;
            let est = estimate_piece_mb(regions, 1.0) as f64;
            assert!(
                est >= peak && est <= peak * 1.3,
                "{regions}: {est} vs {peak}"
            );
        }
    }

    #[test]
    fn a_small_machine_plans_small() {
        // 8 threads, 16 GB: two workers of three threads, not the 24-thread four of five.
        let total = 16 * 1024;
        let usable = 12_000 - reserve_mb(total);
        assert_eq!(
            auto_plan(
                8 * AUTO_CPU_TARGET / 100,
                16,
                usable,
                estimate_piece_mb(64, 1.0)
            ),
            (2, 3)
        );
        // 4 threads, 3 GB to spare: still two, of two threads, on 4 x 4 pieces.
        assert_eq!(auto_max_regions(&parse(&["--ram-budget-mb", "1500"])), 4);
        assert_eq!(
            auto_plan(
                4 * AUTO_CPU_TARGET / 100,
                16,
                1500,
                estimate_piece_mb(16, 1.0)
            ),
            (2, 1)
        );
    }

    #[test]
    fn auto_builds_a_small_selection_in_one_run() {
        let (s8, s16) = (
            "44.3909,26.0522,44.4627,26.1528",
            "44.3549,26.0019,44.4987,26.2031",
        );
        let pieces = |bbox: &str, workers: &str, extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", bbox, "--one-world"];
            cmd.extend(["--unit-regions", "auto", "--one-world-workers", workers]);
            cmd.extend_from_slice(extra);
            let mut args = <Args as clap::Parser>::parse_from(cmd);
            single_below_break_even(&mut args);
            args.units.coordinates()
        };
        // S8 (64 km2) in one run, S16 (256 km2) in pieces.
        assert!(!pieces(s8, "auto", &[]));
        assert!(pieces(s16, "auto", &[]));
        // Another job flag or a fixed count keeps the pieces.
        assert!(pieces(s8, "auto", &["--margin-regions", "1"]));
        assert!(pieces(s8, "auto", &["--prewarm-first"]));
        assert!(pieces(s8, "4", &[]));
    }

    #[test]
    fn builds_single_matches_the_runner() {
        let args = |workers: &str, units: &str| {
            let cmd = [
                "arnis",
                "--output-dir",
                ".",
                "--bbox",
                "44.3909,26.0522,44.4627,26.1528",
                "--one-world",
                "--unit-regions",
                units,
                "--one-world-workers",
                workers,
            ];
            <Args as clap::Parser>::parse_from(cmd)
        };
        let auto = args("auto", "auto");
        assert!(builds_single(&auto, AUTO_SINGLE_BELOW_KM2 - 1.0));
        assert!(!builds_single(&auto, AUTO_SINGLE_BELOW_KM2));
        assert!(!builds_single(&args("4", "auto"), 10.0));
        assert!(!builds_single(&args("auto", "4"), 10.0));
        // The runner's drop agrees with the plan's answer.
        let mut a = args("auto", "auto");
        a.bbox = None;
        single_below_break_even(&mut a);
        assert!(a.units.coordinates()); // no bbox: infinite km2, not single
    }
}
