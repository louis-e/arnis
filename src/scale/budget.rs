//! How many pieces of a job run at once, and what each may use.
//!
//! A fixed count (`--one-world-workers N`) splits the memory one run would take:
//! `--ram-budget-mb` else what is free. Every piece gets all the threads (`--threads` else
//! 90% of the cores), which the system shares between them. Pieces start, and so mostly fetch,
//! one at a time, but may overlap: they share twice a run's downloads (`--max-downloads` else
//! the default), at least 2 each.
//! `auto` picks W = clamp(min(pieces, threads / MIN_THREADS, free RAM / piece RAM), 2, 6), or 1
//! below `TWO_WORKERS_FROM` threads or one piece's RAM: a few fat workers beat many thin ones.

use crate::args::{Args, UnitSize, Workers};
use std::ffi::OsString;

/// Threads below which a piece is not worth its own process.
const MIN_THREADS: usize = 5;
const MIN_WORKERS: usize = 2;
/// Fewer threads build one piece at a time.
const TWO_WORKERS_FROM: usize = 6;
pub const MAX_WORKERS: usize = 6;
/// CPU share planned for, leaving the machine usable.
const CPU_TARGET: usize = 90;

/// Peak memory of one piece of `regions` regions before any has reported. Measured in central
/// Berlin with warm caches and 9 threads a piece: 16 x 16 regions 3,100 MB, 8 x 8 1,290 MB,
/// 4 x 4 810 MB. More threads raise it, which `Sizing::cap_for` catches once a piece reports.
pub fn estimate_piece_mb(regions: u64, scale: f64) -> u64 {
    let per_region = if scale >= 0.5 { 10 } else { 9 };
    660 + per_region * regions
}

/// Every side `auto_max_regions` can give.
pub const AUTO_SIZES: [i32; 5] = [16, 12, 8, 4, 2];

/// Largest side, in regions, that `--unit-regions auto` gives a piece: 16 or 12 when
/// `MAX_WORKERS` such pieces fit `--ram-budget-mb`, else 8 or 4 when two do, else 2. Without a
/// budget, the machine's memory less the reserve: total, so a resumed job cuts the same pieces.
pub fn auto_max_regions(args: &Args) -> i32 {
    let budget = memory_budget_mb(args);
    [
        (16, MAX_WORKERS),
        (12, MAX_WORKERS),
        (8, MIN_WORKERS),
        (4, MIN_WORKERS),
    ]
    .into_iter()
    .find(|&(s, pieces)| estimate_piece_mb(s * s, args.scale) * pieces as u64 <= budget)
    .map_or(2, |(s, _)| s as i32)
}

/// Memory kept free for the system and everything else running.
fn reserve_mb(total_mb: u64) -> u64 {
    (total_mb / 10).max(2048)
}

/// `--ram-budget-mb`, else the machine's memory less the reserve.
fn memory_budget_mb(args: &Args) -> u64 {
    args.ram_budget_mb.unwrap_or_else(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let total = sys.total_memory() / (1024 * 1024);
        total.saturating_sub(reserve_mb(total))
    })
}

/// Ground at scale 1, km², below which `auto` builds one plain run instead of pieces, where
/// the window first warns about the size. Berlin, warm caches, 10 cores, one run vs auto:
/// 100 km² 43.8 s / 3.3 GB vs 37.8-43.6 s / 2.5 GB, 198 km² 85-90 s / 5.2 GB vs 76-84 s / 3.3 GB.
pub const AUTO_SINGLE_BELOW_KM2: f64 = 100.0;

/// Peak memory of one plain run. Berlin, warm caches: 27 km² 1.5 GB, 100 km² 3.3 GB, 198 km²
/// 5.3 GB.
fn estimate_single_mb(km2: f64) -> u64 {
    1000 + (25.0 * km2) as u64
}

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
        && u.skip_cells.is_none()
        && !u.skip_built
        && !u.stitch_map
        && !args.prewarm
        && !args.prewarm_first
        && km2 < AUTO_SINGLE_BELOW_KM2
        && estimate_single_mb(km2) <= memory_budget_mb(args)
}

/// `--unit-regions auto --one-world-workers auto` on a selection smaller than
/// `AUTO_SINGLE_BELOW_KM2`, with no other job flag: drops both, so the One World is
/// built in one plain run, which is faster there. Without `one_world` the flags stay, for
/// the argument check to refuse. The window passes its own: its `Args` keep the field off.
pub fn single_below_break_even(args: &mut Args, one_world: bool) {
    let km2 = args
        .bbox
        .map_or(f64::INFINITY, |b| b.area_km2() * args.scale * args.scale);
    if one_world && builds_single(args, km2) {
        println!(
            "One World: {km2:.0} km² is below {AUTO_SINGLE_BELOW_KM2:.0} km², built in one run"
        );
        args.units.unit_regions = None;
        args.units.one_world_workers = None;
    }
}

/// W for `auto`: two workers at least once the threads and memory allow it, as
/// overlapping two pieces is about as fast as one run on far less memory.
pub fn auto_plan(budget_threads: usize, pieces: usize, usable_mb: u64, piece_mb: u64) -> usize {
    let by_ram = (usable_mb / piece_mb.max(1)) as usize;
    if budget_threads < TWO_WORKERS_FROM || by_ram == 0 {
        return 1;
    }
    let by_cpu = budget_threads / MIN_THREADS;
    pieces
        .min(by_cpu)
        .min(by_ram)
        .clamp(MIN_WORKERS, MAX_WORKERS)
}

#[derive(Debug, PartialEq)]
pub struct Sizing {
    pub workers: usize,
    /// Per piece: the job's whole budget, as pieces are rarely all in their parallel part.
    pub threads: usize,
    pub ram_mb: Option<u64>,
    /// Per piece: a share of twice the job's allowance (`piece_downloads`).
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

    pub fn child_env(&self) -> [(&'static str, String); 1] {
        [("RAYON_NUM_THREADS", self.threads.to_string())]
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

/// Downloads each of `workers` pieces keeps in flight, of a run's `allowance`: twice the
/// allowance in all, as pieces mostly fetch one after another, at least 2 each and never
/// more than one run's.
fn piece_downloads(allowance: u32, workers: usize) -> u32 {
    let share = (2 * allowance as usize / workers.max(1)).max(2);
    share.min(allowance as usize) as u32
}

/// Sizing for a job of `pieces` pieces, the largest `piece_regions` regions.
pub fn sizing(args: &Args, pieces: usize, piece_regions: u64) -> Sizing {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let budget = args
        .thread_count()
        .unwrap_or(cores * CPU_TARGET / 100)
        .max(1);
    let pieces = pieces.max(1);
    let downloads = args
        .max_downloads
        .unwrap_or(crate::net::MAX_CONCURRENT_REQUESTS as u32);
    let piece_mb = estimate_piece_mb(piece_regions, args.scale);
    let given = args.ram_budget_mb.is_some();
    match args.units.one_world_workers {
        Some(Workers::Auto) => {
            let free = args.ram_budget_mb.unwrap_or_else(|| {
                let mut sys = sysinfo::System::new();
                sys.refresh_memory();
                let total = sys.total_memory() / (1024 * 1024);
                crate::data_processing::available_memory_mb().saturating_sub(reserve_mb(total))
            });
            let workers = auto_plan(budget, pieces, free, piece_mb).min(pieces);
            Sizing {
                workers,
                threads: budget,
                ram_mb: piece_share(free, workers, piece_mb, given),
                downloads: piece_downloads(downloads, workers),
                usable_mb: Some(free),
            }
        }
        fixed => {
            let workers = match fixed {
                Some(Workers::N(n)) => n as usize,
                _ => 1,
            }
            .min(pieces);
            let ram_mb = args
                .ram_budget_mb
                .or_else(|| (workers > 1).then(crate::data_processing::available_memory_mb));
            Sizing {
                workers,
                threads: budget,
                ram_mb: ram_mb.and_then(|mb| piece_share(mb, workers, piece_mb, given)),
                downloads: piece_downloads(downloads, workers),
                usable_mb: None,
            }
        }
    }
}

/// A piece's `--ram-budget-mb` from `mb` for all. Less than a piece needs only pins it to one
/// tile thread: a given budget hands the need, a reading that low hands none.
fn piece_share(mb: u64, workers: usize, piece_mb: u64, given: bool) -> Option<u64> {
    let share = mb / workers.max(1) as u64;
    if share >= piece_mb {
        Some(share)
    } else {
        given.then_some(piece_mb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_plans_one_to_six_workers() {
        // 24 cores at 90%: 21 threads, 4 workers.
        assert_eq!(auto_plan(21, 16, 30_000, 976), 4);
        // Never more than six, however big the machine.
        assert_eq!(auto_plan(96, 100, 500_000, 720), 6);
        // Short on pieces, or memory for two: still two. Not for one: one.
        assert_eq!(auto_plan(18, 1, 30_000, 720), 2);
        assert_eq!(auto_plan(18, 16, 1000, 720), 2);
        assert_eq!(auto_plan(18, 16, 100, 720), 1);
        // 10 cores: two; under six threads, one piece at a time.
        assert_eq!(auto_plan(9, 16, 7_000, 976), 2);
        assert_eq!(auto_plan(6, 16, 7_000, 976), 2);
        assert_eq!(auto_plan(5, 16, 500_000, 720), 1);
    }

    #[test]
    fn a_piece_never_gets_less_memory_than_it_needs() {
        assert_eq!(piece_share(8000, 4, 544, false), Some(2000));
        // A reading that low passes none, a given budget the need.
        assert_eq!(piece_share(1, 2, 976, false), None);
        assert_eq!(piece_share(1000, 2, 976, true), Some(976));
    }

    #[test]
    fn piece_memory_follows_its_size_and_scale() {
        assert_eq!(estimate_piece_mb(16, 1.0), 820);
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
    fn auto_pieces_are_as_large_as_the_memory_fits() {
        // Six 16 x 16 pieces at scale 1 need 6 x 3220 MB, six 12 x 12 6 x 2100 MB; two
        // 8 x 8 2 x 1300 MB, two 4 x 4 2 x 820 MB.
        for (mb, side) in [
            (19320, 16),
            (19319, 12),
            (12600, 12),
            (12599, 8),
            (4000, 8),
            (2600, 8),
            (2599, 4),
            (1640, 4),
            (1639, 2),
            (10, 2),
        ] {
            let args = parse(&["--ram-budget-mb", &mb.to_string()]);
            assert_eq!(auto_max_regions(&args), side, "{mb} MB");
            assert!(AUTO_SIZES.contains(&side));
        }
    }

    #[test]
    fn pieces_share_twice_a_runs_downloads() {
        // One or two at a time: each the run's 16; six: 5 each, 30 in all.
        assert_eq!(piece_downloads(16, 1), 16);
        assert_eq!(piece_downloads(16, 2), 16);
        assert_eq!(piece_downloads(16, 4), 8);
        assert_eq!(piece_downloads(16, 6), 5);
        // Never under 2 a piece, unless the run itself is allowed less.
        assert_eq!(piece_downloads(16, 64), 2);
        assert_eq!(piece_downloads(1, 4), 1);
        let s = sizing(
            &parse(&["--one-world-workers", "6", "--max-downloads", "8"]),
            16,
            16,
        );
        assert_eq!((s.workers, s.downloads), (6, 2));
    }

    #[test]
    fn a_fixed_count_splits_memory_and_downloads_not_threads() {
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
                threads: 20,
                ram_mb: Some(2000),
                downloads: 8,
                usable_mb: None,
            }
        );
        // The stock region writers, not a count cut down per piece.
        assert!(s
            .child_env()
            .iter()
            .all(|(k, _)| *k != "ARNIS_FLUSH_THREADS"));
        // Never more workers than pieces.
        let s = sizing(
            &parse(&["--one-world-workers", "8", "--threads", "3"]),
            2,
            16,
        );
        assert_eq!((s.workers, s.threads), (2, 3));
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
        // 16 threads allow 3 workers, 6000 MB at 820 MB a piece allow 7.
        assert_eq!((s.workers, s.threads, s.ram_mb), (3, 16, Some(2000)));
        assert_eq!(s.usable_mb, Some(6000));
        // Less than one piece needs: one at a time, never with less than that need.
        let s = sizing(
            &parse(&[
                "--one-world-workers",
                "auto",
                "--threads",
                "16",
                "--ram-budget-mb",
                "300",
            ]),
            16,
            16,
        );
        assert_eq!((s.workers, s.threads, s.ram_mb), (1, 16, Some(820)));
        assert_eq!(s.usable_mb, Some(300));
    }

    // 256 km2 of Bucharest, warm caches, 24 threads, 31 GB: fastest at 8 x 8 regions with 4
    // workers, peak 3,298 MB for the job; 4 x 4 regions with 4 workers peaked at 2,116 MB.
    #[test]
    fn auto_plan_picks_measured_sweet_spot() {
        let total = 31 * 1024;
        let usable = total - reserve_mb(total);
        // 16 pieces of up to 8 x 8.
        let plan = auto_plan(
            24 * CPU_TARGET / 100,
            16,
            usable,
            estimate_piece_mb(64, 1.0),
        );
        assert_eq!(plan, 4);
    }

    // The largest peak a piece of central Berlin reported with 9 threads; 16 x 16 is one run.
    #[test]
    fn estimate_tracks_measured_peak() {
        for (regions, peak) in [(256, 3100.0), (64, 1290.0), (16, 810.0)] {
            let est = estimate_piece_mb(regions, 1.0) as f64;
            assert!(
                est >= peak && est <= peak * 1.3,
                "{regions}: {est} vs {peak}"
            );
        }
    }

    #[test]
    fn a_small_machine_plans_small() {
        // 8 threads, 16 GB: two workers, not the 24-thread four.
        let total = 16 * 1024;
        let usable = 12_000 - reserve_mb(total);
        assert_eq!(
            auto_plan(8 * CPU_TARGET / 100, 16, usable, estimate_piece_mb(64, 1.0)),
            2
        );
        // 4 cores, 3 GB to spare: one piece at a time.
        assert_eq!(auto_max_regions(&parse(&["--ram-budget-mb", "2000"])), 4);
        assert_eq!(
            auto_plan(4 * CPU_TARGET / 100, 16, 2000, estimate_piece_mb(16, 1.0)),
            1
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
            single_below_break_even(&mut args, true);
            args.units.coordinates()
        };
        // 64 km2 in one run, 256 km2 in pieces.
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
        let mut auto = args("auto", "auto");
        auto.ram_budget_mb = Some(8000);
        assert!(builds_single(&auto, AUTO_SINGLE_BELOW_KM2 - 1.0));
        assert!(!builds_single(&auto, AUTO_SINGLE_BELOW_KM2));
        // One run of 60 km² needs about 2.5 GB: short of that, pieces.
        auto.ram_budget_mb = Some(2000);
        assert!(builds_single(&auto, 30.0));
        assert!(!builds_single(&auto, 60.0));
        assert!(!builds_single(&args("4", "auto"), 10.0));
        assert!(!builds_single(&args("auto", "4"), 10.0));
        // Cells left out need the cells.
        auto.units.skip_cells = Some([3].into());
        assert!(!builds_single(&auto, 10.0));
        // The runner's drop agrees with the plan's answer.
        let mut a = args("auto", "auto");
        a.bbox = None;
        single_below_break_even(&mut a, true);
        assert!(a.units.coordinates()); // no bbox: infinite km2, not single
                                        // Without --one-world the flags stay, and the argument check refuses them.
        let mut a = args("auto", "auto");
        a.one_world = false;
        a.ram_budget_mb = Some(8000);
        single_below_break_even(&mut a, false);
        assert!(crate::args::validate_args(&a).is_err());
        a.one_world = true;
        single_below_break_even(&mut a, true);
        assert!(!a.units.coordinates());
    }
}
