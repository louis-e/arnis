//! Large One World jobs (`--unit-regions`, `--one-world-workers`): the selection is cut into
//! region-group pieces (`work_units`), each built by a run of this executable
//! (`--one-world-unit <lease>`), several at once. A piece owns whole region files; the head
//! piece makes the world's extras and starts first; only the coordinator writes the manifest.
//! State lives in `arnis_one_world/jobs/<x0>_<z0>_<x1>_<z1>_n<N>/`, so the same command
//! resumes the job.

use crate::args::{Args, UnitSize};
use crate::coordinate_system::cartesian::XZBBox;
use crate::one_world::{self, Manifest, PieceRef, RunContext, UnitLease};
use crate::progress_json;
use crate::work_units::{self, WorkUnit};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub mod budget;
pub mod child;

/// Map ids each piece may use. A piece needing more would run into the next one's range.
pub(crate) const MAP_IDS_PER_PIECE: i32 = 1 << 16;
/// Pieces one job's map ids have room for in a fresh world.
pub const MAX_PIECES: usize =
    ((i32::MAX - crate::map_item::FIRST_MAP_ID) / MAP_IDS_PER_PIECE) as usize;

/// Pieces whose map ids fit from `next` up.
fn pieces_with_map_ids(next: i32) -> usize {
    (i32::MAX.saturating_sub(next) / MAP_IDS_PER_PIECE) as usize
}
/// Lines of a failed piece's output shown with its error.
const TAIL_LINES: usize = 40;
/// Further attempts at a piece that failed for a reason that may pass.
const MAX_RETRIES: u32 = 2;
/// Piece size when only `--one-world-workers` is given.
pub const DEFAULT_UNIT_REGIONS: i32 = 8;
/// Percent of the job's bar `--prewarm-first` fills before the build, as downloads take of a run.
const WARM_SHARE: f64 = 20.0;
/// A piece's progress at "Generating area...": from there it builds on every thread.
const PARALLEL_FROM: f64 = 0.2;

/// Pause holds the queue: no piece starts, the running ones finish, and the job stays
/// resumable. Stop ends the job and kills its running pieces; the finished ones are kept,
/// so the same run again resumes. Set by whatever drives this process (the window).
pub static PAUSE: AtomicBool = AtomicBool::new(false);
pub static STOP: AtomicBool = AtomicBool::new(false);

/// What a job returns when Stop ended it.
pub const STOPPED: &str = "Stopped.";

/// What a job returns when it built the pieces `--only-pieces` named and others are still
/// to build: not a failure, and the next run finishes it.
pub const PARTIAL: &str =
    "The chosen pieces are built; the others wait for the next run of this job.";

/// Opens the status line a resumed job sends when its settings differ from the ones it was
/// started with, followed by the changed options' names; the window picks it out by this.
pub const SETTINGS_CHANGED: &str = "Settings changed since this job started:";

/// One writer at a time to the manifest, which pieces finish into side by side.
static MANIFEST: Mutex<()> = Mutex::new(());

/// Where each cell (a piece, by plan index) of the running job stands. The window changes it
/// through [`control`] while the job runs; built cells are in no set.
#[derive(Default, Debug)]
pub struct Queue {
    /// Waiting to start, in build order.
    pending: VecDeque<usize>,
    running: BTreeSet<usize>,
    /// Running cells not yet building in parallel. The next cell starts once none is left,
    /// so one cell's fetching and parsing runs beside another's building.
    prologue: BTreeSet<usize>,
    /// Pending cells that do not start until resumed. A held running cell finishes.
    held: BTreeSet<usize>,
    failed: BTreeSet<usize>,
    /// Left out of the job (`--skip-cells`), kept in its state.
    skipped: BTreeSet<usize>,
    /// `skipped` changed since the job last saved it.
    skip_changed: bool,
    /// Running cells to kill and put back in `pending`.
    cancel: BTreeSet<usize>,
    /// State changes the job has not reported yet.
    events: Vec<(usize, &'static str)>,
    /// No worker takes a cell any more, so actions would be lost.
    closed: bool,
}

/// What a worker does next.
#[derive(Debug, PartialEq)]
enum Next {
    Cell(usize),
    Wait,
    Done,
}

/// How a started cell ended.
enum Outcome {
    Built,
    Failed,
    /// Killed by a cancel: back to pending.
    Cancelled,
    Stopped,
}

impl Queue {
    /// The first pending cell that is not held, now running, if a cell may `start`. The job
    /// is done when nothing is pending or running; held cells keep it waiting.
    fn next(&mut self, start: bool) -> Next {
        if self.pending.is_empty() && self.running.is_empty() {
            self.closed = true;
            return Next::Done;
        }
        let at = self.pending.iter().position(|i| !self.held.contains(i));
        match at.filter(|_| start).and_then(|p| self.pending.remove(p)) {
            Some(i) => {
                self.running.insert(i);
                self.prologue.insert(i);
                Next::Cell(i)
            }
            None => Next::Wait,
        }
    }

    /// Whether a cell may start: fewer than `cap` run, none of them in its prologue.
    fn admits(&self, cap: usize) -> bool {
        self.prologue.is_empty() && self.running.len() < cap
    }

    /// Cell `i` ended; the state the window shows for it when it goes back to pending.
    fn finish(&mut self, i: usize, how: Outcome) -> Option<&'static str> {
        self.running.remove(&i);
        self.prologue.remove(&i);
        self.cancel.remove(&i);
        if let Outcome::Cancelled = how {
            self.pending.push_back(i);
            return Some(self.waiting_state(i));
        }
        // Held matters to a waiting cell only.
        self.held.remove(&i);
        if let Outcome::Failed = how {
            self.failed.insert(i);
        }
        None
    }

    fn waiting_state(&self, i: usize) -> &'static str {
        if self.held.contains(&i) {
            "held"
        } else {
            "queued"
        }
    }

    fn cancelled(&self, i: usize) -> bool {
        self.cancel.contains(&i)
    }

    /// Applies `action` to `cells`; returns the cells it changed, the rest are ignored.
    fn apply(&mut self, action: &str, cells: &[usize]) -> Result<Vec<usize>, String> {
        let mut applied = Vec::new();
        let pending = |q: &Self, i: usize| q.pending.contains(&i);
        match action {
            "hold" => {
                for &i in cells {
                    if (pending(self, i) || self.running.contains(&i)) && self.held.insert(i) {
                        if pending(self, i) {
                            self.events.push((i, "held"));
                        }
                        applied.push(i);
                    }
                }
            }
            "resume" => {
                for &i in cells {
                    if self.held.remove(&i) {
                        if pending(self, i) {
                            self.events.push((i, "queued"));
                        }
                        applied.push(i);
                    }
                }
            }
            "front" => {
                // The given order, first at the front.
                for &i in cells.iter().rev() {
                    if let Some(p) = self.pending.iter().position(|&c| c == i) {
                        self.pending.remove(p);
                        self.pending.push_front(i);
                        applied.push(i);
                    }
                }
                applied.reverse();
            }
            "cancel" => {
                for &i in cells {
                    if self.running.contains(&i) && self.cancel.insert(i) {
                        applied.push(i);
                    }
                }
            }
            "skip" => {
                for &i in cells {
                    if let Some(p) = self.pending.iter().position(|&c| c == i) {
                        self.pending.remove(p);
                        self.held.remove(&i);
                        self.skipped.insert(i);
                        self.events.push((i, "skipped"));
                        applied.push(i);
                    }
                }
                self.skip_changed |= !applied.is_empty();
            }
            "include" => {
                for &i in cells {
                    if self.skipped.remove(&i) {
                        self.pending.push_back(i);
                        self.events.push((i, "queued"));
                        applied.push(i);
                    }
                }
                self.skip_changed |= !applied.is_empty();
            }
            "retry" => {
                for &i in cells {
                    if self.failed.remove(&i) {
                        self.pending.push_back(i);
                        self.events.push((i, self.waiting_state(i)));
                        applied.push(i);
                    }
                }
            }
            _ => {
                return Err(format!(
                "{action}: not a cell action (hold, resume, front, cancel, retry, skip, include)."
            ))
            }
        }
        Ok(applied)
    }
}

/// What [`control`] did, and where the job's cells stand after it.
#[derive(serde::Serialize, Debug)]
pub struct CellsControlResult {
    pub applied: Vec<u32>,
    /// Cells the action does not apply to in their state (say, cancel on a pending cell).
    pub ignored: Vec<u32>,
    pub pending: Vec<u32>,
    pub running: Vec<u32>,
    pub held: Vec<u32>,
    pub failed: Vec<u32>,
    pub skipped: Vec<u32>,
}

/// The running job's world and cells; one job per process.
static ACTIVE: Mutex<Option<(PathBuf, Arc<Mutex<Queue>>)>> = Mutex::new(None);

/// Clears [`ACTIVE`] however the job ends.
struct Active;

impl Active {
    fn register(world_dir: &Path, queue: &Arc<Mutex<Queue>>) -> Self {
        *lock(&ACTIVE) = Some((world_dir.to_path_buf(), Arc::clone(queue)));
        Active
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        *lock(&ACTIVE) = None;
    }
}

/// Whether a job of this process builds the world at `world_dir`.
pub fn runs_on(world_dir: &Path) -> bool {
    let same = |a: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(world_dir)
    };
    lock(&ACTIVE).as_ref().is_some_and(|(w, _)| same(w))
}

/// The window's cell actions on the running job: `hold` keeps pending cells from starting
/// (a running one finishes), `resume` lets them, `front` moves pending cells to the front in
/// the given order, `cancel` kills running cells and puts them back in pending, `retry`
/// puts failed cells back in pending, `skip` leaves pending cells out of the job and
/// `include` puts skipped cells back in pending.
pub fn control(action: &str, cells: &[u32]) -> Result<CellsControlResult, String> {
    let active = lock(&ACTIVE);
    let (_, queue) = active.as_ref().ok_or("No job is running.")?;
    let mut q = lock(queue);
    if q.closed {
        return Err("The job is finishing; no cell can change now.".to_string());
    }
    let wanted: Vec<usize> = cells.iter().map(|&c| c as usize).collect();
    let applied = q.apply(action, &wanted)?;
    let ids = |s: &mut dyn Iterator<Item = &usize>| s.map(|&i| i as u32).collect::<Vec<_>>();
    Ok(CellsControlResult {
        ignored: cells
            .iter()
            .copied()
            .filter(|c| !applied.contains(&(*c as usize)))
            .collect(),
        applied: ids(&mut applied.iter()),
        pending: ids(&mut q.pending.iter()),
        running: ids(&mut q.running.iter()),
        held: ids(&mut q.held.iter()),
        failed: ids(&mut q.failed.iter()),
        skipped: ids(&mut q.skipped.iter()),
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a finished piece reported.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PieceResult {
    pub peak_rss_mb: Option<u64>,
    pub wall_s: Option<f64>,
    pub chunks: u64,
}

/// One job's folder and the ids it reserved, kept across a resume.
struct Job {
    name: String,
    dir: PathBuf,
    area_base: u32,
    /// Each piece's share of the reserved area ids, by region key.
    slots: serde_json::Map<String, Value>,
    /// False in a prewarm, which takes no ids.
    reserve: bool,
    /// Cells left out, by plan index: `--skip-cells`, kept in `job.json`.
    skip: BTreeSet<usize>,
    /// The head piece an earlier run chose and did not build, kept in `job.json`.
    head: Option<usize>,
    /// Cells built by an earlier run whose record went with the job folder (`built_cells`).
    recorded: BTreeSet<usize>,
}

type Slots = serde_json::Map<String, Value>;

/// The area ids a kept job state reserved, `None` when it holds none. Numbers no piece's ids
/// fit in mean a damaged file: an error, where using them would overflow.
fn reserved_ids(v: &Value, state: &Path) -> Result<Option<(u32, Slots)>, String> {
    let (Some(a), Some(slots)) = (v["area_base"].as_u64(), v["slots"].as_object()) else {
        return Ok(None);
    };
    let damaged = || {
        format!(
            "{} is damaged; delete it and run the job again.",
            state.display()
        )
    };
    let area_base = u32::try_from(a).map_err(|_| damaged())?;
    for slot in slots.values() {
        let s = slot.as_u64().ok_or_else(damaged)?;
        let fits = u32::try_from(s).ok().and_then(|s| area_base.checked_add(s));
        if fits.is_none() {
            return Err(damaged());
        }
    }
    Ok(Some((area_base, slots.clone())))
}

/// A finished piece's file names its regions and size, so it stays its own whatever order
/// the pieces build in: `done-r<rx>_<rz>_s<n>.json`.
fn done_key(u: &WorkUnit) -> String {
    format!(
        "r{}",
        u.region_key().replacen(',', "_", 1).replacen(',', "_s", 1)
    )
}

impl Job {
    /// `options` are what decides the world (`generation_options`): kept with a new job, and
    /// a resume with others is warned about. Without `reserve` (a prewarm) a new job reserves
    /// no ids and writes no state.
    fn open(
        world_dir: &Path,
        name: String,
        units: &[WorkUnit],
        options: &[String],
        reserve: bool,
    ) -> Result<Self, String> {
        let pieces = units.len();
        let dir = world_dir.join(one_world::JOBS_DIR).join(&name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
        let state = dir.join("job.json");
        let kept = std::fs::read_to_string(&state)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let reserved = kept.as_ref().map(|v| reserved_ids(v, &state));
        let (area_base, slots) = match reserved.transpose()?.flatten() {
            Some((a, slots)) => {
                if let Some(u) = units.iter().find(|u| !slots.contains_key(&done_key(u))) {
                    return Err(format!(
                        "{} was planned with other pieces (none at region {}); delete it or run \
                         the command that started it.",
                        dir.display(),
                        u.region_key()
                    ));
                }
                let started: Vec<String> = kept.as_ref().map_or_else(Vec::new, |v| {
                    v["argv"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|o| o.as_str().map(str::to_owned))
                        .collect()
                });
                let changed = changed_options(&started, options);
                if let Some(notice) = settings_changed_notice(&changed) {
                    eprintln!(
                        "Warning: this job was started with other settings ({}); the pieces \
                         still to build use the new ones.",
                        changed.join(", ")
                    );
                    // The window has no stderr. A prewarm builds nothing, so it has no news.
                    if reserve {
                        crate::progress::emit_gui_progress_update(
                            crate::progress::MESSAGE_ONLY,
                            &notice,
                        );
                    }
                }
                (a, slots)
            }
            _ => {
                // Reserved once, so a run in between cannot take a piece's area id.
                let _guard = lock(&MANIFEST);
                let mut manifest = Manifest::load(world_dir)?.ok_or("The One World vanished.")?;
                let fresh = starts_over(&manifest, &name, None);
                let area_base = manifest.next_area_id;
                let slots: serde_json::Map<String, Value> = units
                    .iter()
                    .map(|u| (done_key(u), json!(u.index)))
                    .collect();
                if reserve {
                    manifest.next_area_id += pieces as u32;
                    manifest.save(world_dir)?;
                    write(
                        &state,
                        &json!({ "area_base": area_base, "slots": slots, "argv": options,
                                 "fresh": fresh }),
                    )?;
                }
                (area_base, slots)
            }
        };
        let skip = kept
            .as_ref()
            .map_or_else(BTreeSet::new, |v| skip_set(&v["skip"]));
        let head = kept
            .as_ref()
            .and_then(|v| v["head"].as_u64())
            .map(|h| h as usize);
        let recorded = built_cells(world_dir, &name);
        Ok(Self {
            name,
            dir,
            area_base,
            slots,
            reserve,
            skip,
            head,
            recorded,
        })
    }

    /// Keeps `skip` in `job.json`, so a run of the same job leaves the same cells out.
    fn save_skip(&mut self, skip: BTreeSet<usize>) -> Result<(), String> {
        save_skip(&self.dir, &skip)?;
        self.skip = skip;
        Ok(())
    }

    /// The piece's share of the job's area ids.
    fn slot(&self, u: &WorkUnit) -> u32 {
        self.slots
            .get(&done_key(u))
            .and_then(Value::as_u64)
            .map_or(u.index, |s| s as u32)
    }

    fn area_id(&self, u: &WorkUnit) -> u32 {
        self.area_base + self.slot(u)
    }

    /// The map ids piece `i` took when an earlier run started it, from its lease.
    fn kept_map_id(&self, i: usize) -> Option<i32> {
        let lease = std::fs::read_to_string(self.lease_path(i)).ok()?;
        let first = serde_json::from_str::<UnitLease>(&lease).ok()?.first_map_id;
        let range = crate::map_item::FIRST_MAP_ID..=i32::MAX - MAP_IDS_PER_PIECE + 1;
        range.contains(&first).then_some(first)
    }

    /// Piece `i`'s own range of map ids: the one it has, else the next off the world's
    /// counter, taken as it starts, so the world spends ids only on the cells a run builds.
    fn first_map_id(&self, world_dir: &Path, i: usize) -> Result<i32, String> {
        static COUNTER: Mutex<()> = Mutex::new(());
        if let Some(first) = self.kept_map_id(i) {
            return Ok(first);
        }
        // A prewarm writes no maps.
        if !self.reserve {
            return Ok(0);
        }
        let _guard = lock(&COUNTER);
        let maps = crate::world_utils::WorldLayout::of(world_dir).maps_dir(world_dir);
        let first = crate::map_item::next_map_id(&maps);
        if pieces_with_map_ids(first) == 0 {
            return Err(
                "This world has no map ids left for another piece; use a new One World."
                    .to_string(),
            );
        }
        // Before the piece starts: no other piece or run may be handed these ids.
        crate::map_item::set_map_counter(world_dir, first + MAP_IDS_PER_PIECE - 1)?;
        Ok(first)
    }

    fn lease_path(&self, piece: usize) -> PathBuf {
        self.dir.join(format!("piece-{piece}.lease.json"))
    }

    /// `--skip-built`: every chunk of the piece is in the world already, from an earlier
    /// job. Never a piece this job started, nor one over an `unfinished` piece of another
    /// job: their chunks may be half written.
    fn built_before(&self, world_dir: &Path, u: &WorkUnit, unfinished: &[[i32; 4]]) -> bool {
        !self.lease_path(u.index as usize).exists()
            && !unfinished
                .iter()
                .any(|r| one_world::overlaps(r, &corners(&u.rect)))
            && one_world::existing_chunks(world_dir, &u.rect) == u.chunks()
    }

    fn done_path(&self, u: &WorkUnit) -> PathBuf {
        self.dir.join(format!("done-{}.json", done_key(u)))
    }

    /// What the piece reported, if it is built.
    fn finished(&self, u: &WorkUnit) -> Option<PieceResult> {
        std::fs::read_to_string(self.done_path(u))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .or_else(|| {
                let recorded = self.recorded.contains(&(u.index as usize));
                recorded.then(PieceResult::default)
            })
    }
}

/// Whether a run of job `name` builds every cell again: a job that finished, as its folder
/// (`kept` is its state) noted when the run began, else as the manifest says.
fn starts_over(manifest: &Manifest, name: &str, kept: Option<&Value>) -> bool {
    match kept.filter(|v| v["slots"].is_object()) {
        Some(v) => v["fresh"] == true,
        None => manifest
            .jobs
            .iter()
            .any(|j| j.name == name && j.status.finished),
    }
}

/// The rectangles of pieces a job in `world_dir` started and did not finish: a lease and no
/// done file. Every chunk of one may be there, half of them from before.
fn unfinished_pieces(world_dir: &Path) -> Vec<[i32; 4]> {
    let mut out = Vec::new();
    let jobs = std::fs::read_dir(world_dir.join(one_world::JOBS_DIR));
    for dir in jobs.into_iter().flatten().flatten().map(|e| e.path()) {
        let files: Vec<String> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        for name in files.iter().filter(|n| n.ends_with(".lease.json")) {
            let lease = std::fs::read_to_string(dir.join(name)).ok();
            let Some(rect) = lease.and_then(|t| serde_json::from_str::<UnitLease>(&t).ok()) else {
                continue;
            };
            let r = rect.rect;
            let done = format!("done-r{}_{}_s", r[0].div_euclid(512), r[1].div_euclid(512));
            if !files.iter().any(|f| f.starts_with(&done)) {
                out.push(r);
            }
        }
    }
    out
}

/// Cells of job `name` the manifest holds an area for, by plan index: built, though the job
/// folder that recorded them is gone. None for a job that starts over.
pub fn built_cells(world_dir: &Path, name: &str) -> BTreeSet<usize> {
    let Ok(Some(manifest)) = Manifest::load(world_dir) else {
        return BTreeSet::new();
    };
    let state = world_dir
        .join(one_world::JOBS_DIR)
        .join(name)
        .join("job.json");
    let kept = std::fs::read_to_string(state)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if starts_over(&manifest, name, kept.as_ref()) {
        return BTreeSet::new();
    }
    let pieces = manifest.areas.iter().filter_map(|a| a.piece.as_ref());
    pieces
        .filter(|p| p.job == name)
        .map(|p| p.index as usize)
        .collect()
}

/// Whether the job folder `name` holds `u` as built, read as a resume reads it.
pub fn piece_done(world_dir: &Path, name: &str, u: &WorkUnit) -> bool {
    let dir = world_dir.join(one_world::JOBS_DIR).join(name);
    dir.join(format!("done-{}.json", done_key(u))).exists()
}

fn skip_set(v: &Value) -> BTreeSet<usize> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c.as_u64().map(|c| c as usize))
        .collect()
}

/// Writes the cells left out into the job state in `dir`.
fn save_skip(dir: &Path, skip: &BTreeSet<usize>) -> Result<(), String> {
    save_state(dir, "skip", json!(skip))
}

/// Sets `key` in the job state in `dir`. One writer at a time: workers and the job's loop.
fn save_state(dir: &Path, key: &str, value: Value) -> Result<(), String> {
    static STATE: Mutex<()> = Mutex::new(());
    let _guard = lock(&STATE);
    let state = dir.join("job.json");
    let mut v: Value = std::fs::read_to_string(&state)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    v[key] = value;
    write(&state, &v)
}

/// The cells a run of job `name` leaves out: `--skip-cells` when given, else those kept in
/// the job's state.
pub fn skipped_cells(world_dir: &Path, name: &str, args: &Args) -> BTreeSet<usize> {
    if let Some(cells) = &args.units.skip_cells {
        return cells.iter().map(|&c| c as usize).collect();
    }
    let state = world_dir
        .join(one_world::JOBS_DIR)
        .join(name)
        .join("job.json");
    std::fs::read_to_string(state)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map_or_else(BTreeSet::new, |v| skip_set(&v["skip"]))
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    crate::world_utils::replace_file_atomically(path, value.to_string().as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Builds the selection `args` was opened on, piece by piece. Each piece runs this
/// command line again with its own bbox and lease.
pub fn run(args: &Args, world_dir: &Path) -> Result<(), String> {
    // The window clears them in its start command, where a click may already have set them.
    PAUSE.store(false, Ordering::Release);
    STOP.store(false, Ordering::Release);
    run_with(args, world_dir, std::env::args_os().skip(1).collect(), &[])
}

/// Waits while Pause holds the job; false once Stop ends it.
fn wait_while_paused() -> bool {
    while PAUSE.load(Ordering::Acquire) && !STOP.load(Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    !STOP.load(Ordering::Acquire)
}

/// `run` with the command line the pieces repeat; the window has none of its own to give.
/// `settings` are the window's flags, kept with the job so it can be run again.
pub fn run_with(
    args: &Args,
    world_dir: &Path,
    argv: Vec<OsString>,
    settings: &[String],
) -> Result<(), String> {
    let ctx = args
        .one_world_run
        .as_ref()
        .ok_or("--unit-regions needs --one-world")?;
    let selection = args.bbox.ok_or("A job built in pieces needs a bbox.")?;
    let size = args
        .units
        .unit_regions
        .unwrap_or(UnitSize::N(DEFAULT_UNIT_REGIONS));
    let manifest = Manifest::load(world_dir)?
        .ok_or_else(|| format!("{} is not a One World.", world_dir.display()))?;
    let proj = manifest.projection();
    let plan = work_units::plan_in(world_dir, &proj, &selection, args, size)?;
    // A snapped job reaches past the selection, and its data must too.
    let bake_area = if corners(&plan.rect) == corners(&plan.requested) {
        selection
    } else {
        crate::projection::llbbox_for_rect(&proj, &plan.rect)?
    };
    let asked = kept_bbox(&proj, &plan.requested);
    let (rect, units) = (plan.rect, plan.units);
    let of = units.len();
    // `--only-pieces`: these are built, the job's other missing pieces wait.
    let only = args.units.only_pieces.as_deref();
    if let Some(&bad) = only.into_iter().flatten().find(|&&p| p as usize > of) {
        return Err(format!("--only-pieces {bad}: this job has {of} pieces."));
    }
    if let Some(&bad) = args
        .units
        .skip_cells
        .iter()
        .flatten()
        .find(|&&c| c as usize >= of)
    {
        return Err(format!(
            "--skip-cells {bad}: this job's cells are 0 to {}.",
            of - 1
        ));
    }
    let order = work_units::build_order(&units, args.units.piece_order.as_ref())?;
    // Snapping or a mask can leave the requested spawn outside every piece.
    if let Some((x, z)) = user_spawn_xz(args, &selection)
        .filter(|&(x, z)| !units.iter().any(|u| contains(&u.rect, x, z)))
    {
        eprintln!(
            "Warning: the spawn point (--spawn-lat/--spawn-lng, block {x}, {z}) is in no piece \
             of this job and is ignored."
        );
    }
    let job_existed = world_dir
        .join(one_world::JOBS_DIR)
        .join(&plan.name)
        .exists();
    // A prewarm writes no world: no ids reserved, no job state.
    let mut job = Job::open(
        world_dir,
        plan.name,
        &units,
        &generation_options(&argv),
        !args.prewarm,
    )?;
    // `--skip-cells` replaces the cells the job kept out, an empty list none.
    if let Some(cells) = &args.units.skip_cells {
        let cells: BTreeSet<usize> = cells.iter().map(|&c| c as usize).collect();
        if args.prewarm {
            job.skip = cells;
        } else {
            job.save_skip(cells)?;
        }
    }
    let job = job;
    let unfinished = if args.units.skip_built {
        unfinished_pieces(world_dir)
    } else {
        Vec::new()
    };
    let wanted =
        |i: usize| !job.skip.contains(&i) && only.is_none_or(|p| p.contains(&(i as u32 + 1)));

    let side = units.iter().map(|u| u.size).max().unwrap_or(1);
    println!("One World: building {of} piece(s) of up to {side}x{side} regions");
    // The extract is read once, here; each piece then cuts its own area from the bake.
    let mut argv = argv;
    if let Some(src) = crate::osm_pbf::Source::from_args(args).filter(|_| !args.skip_osm()) {
        match crate::osm_pbf::bake_for_job(&src, bake_area) {
            Err(e) if crate::osm_pbf::fallback_allowed(&src, crate::net::offline()) => {
                crate::retrieve_data::warn_region_download_fallback(&e);
                argv = without_options(argv, &["--osm-pbf", "--osm-pbf-url"], &[]);
            }
            r => r?,
        }
    }
    let results: Mutex<Vec<Option<PieceResult>>> = Mutex::new(vec![None; of]);
    let mut queue = VecDeque::new();
    let piece_record = |i: usize, state: &str, extra: Value| {
        // The area the piece builds beyond its own: its halo, for the window's map.
        let halo = crate::projection::llbbox_for_rect(
            &proj,
            &one_world::piece_build_rect(&units[i].rect, &rect),
        )
        .ok()
        .map(|b| [b.min().lat(), b.min().lng(), b.max().lat(), b.max().lng()]);
        let mut body = json!({"piece": i, "of": of, "state": state,
                              "rect": corners(&units[i].rect), "bounds": bounds(&units[i]),
                              "halo": halo, "log": piece_log_path(&job.dir, i)});
        if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
            body.extend(extra);
        }
        crate::progress::emit_gui_piece(&body);
        // `queued` is for the window's map; the NDJSON stream keeps the states it had.
        if state != "queued" {
            progress_json::record("piece", body);
        }
    };
    // How far a running piece is, for its outline in the window. Not in the NDJSON stream: that
    // keeps one record per state change.
    let piece_progress = |i: usize, state: &str, worker: usize, pct: f64| {
        crate::progress::emit_gui_piece(&json!({
            "piece": i, "of": of, "state": state, "rect": corners(&units[i].rect),
            "bounds": bounds(&units[i]), "worker": worker, "pct": pct.round(),
        }));
    };
    for i in order {
        let unit = &units[i];
        match job.finished(unit) {
            Some(r) => {
                println!("  piece {}/{of}: already built", i + 1);
                piece_record(i, "skipped", json!({}));
                lock(&results)[i] = Some(r);
            }
            None if job.skip.contains(&i) => {
                println!("  piece {}/{of}: left out", i + 1);
                piece_record(i, "skipped", json!({"reason": "skip_cells"}));
            }
            // `--skip-built`: every chunk is in the world already, from an earlier job.
            None if wanted(i)
                && args.units.skip_built
                && job.built_before(world_dir, unit, &unfinished) =>
            {
                println!(
                    "  piece {}/{of}: every chunk already exists, skipped",
                    i + 1
                );
                piece_record(i, "skipped", json!({}));
                lock(&results)[i] = Some(PieceResult::default());
            }
            None if wanted(i) => {
                piece_record(i, "queued", json!({}));
                queue.push_back(i);
            }
            None => {}
        }
    }

    // Only the piece that holds the spawn gets it: a run refuses a spawn outside its area.
    let spawn_piece = spawn_piece(&units, user_spawn_xz(args, &selection));
    // A fresh world gets its map item, settings and spawn from one piece, the head, which
    // goes first whatever `--piece-order` says. A head an earlier run of the job chose and
    // did not build is still owed: other pieces may be recorded by now.
    let owed = job
        .head
        .filter(|&h| h < of && job.finished(&units[h]).is_none());
    let head =
        head_position(&queue, owed, spawn_piece, !ctx.extending).and_then(|at| queue.remove(at));
    if head.is_some() && head != job.head && !args.prewarm {
        save_state(&job.dir, "head", json!(head))?;
    }

    // Refused before any piece starts, not when the ids run out halfway.
    let to_build = head.iter().chain(&queue);
    let need = to_build.filter(|&&i| job.kept_map_id(i).is_none()).count();
    let maps = crate::world_utils::WorldLayout::of(world_dir).maps_dir(world_dir);
    let room = pieces_with_map_ids(crate::map_item::next_map_id(&maps));
    if need > room && !args.prewarm {
        return Err(format!(
            "This world has map ids left for {room} pieces and this run builds {need}; use \
             larger pieces (--unit-regions) or a new One World."
        ));
    }

    let largest = units.iter().map(|u| u.regions).max().unwrap_or(1);
    let sizing = budget::sizing(args, queue.len() + usize::from(head.is_some()), largest);
    if sizing.workers > 1 {
        println!(
            "  {} pieces at a time, {} threads each",
            sizing.workers, sizing.threads
        );
    }
    // `--mapillary-token` reads MAPILLARY_TOKEN, so a piece gets it without a command line.
    // A piece's own environment does not count (`PIECE_ENV`), so this run's offline and
    // Overpass settings go along too.
    let token: Vec<(&str, String)> = args
        .mapillary_api_token()
        .map(|t| ("MAPILLARY_TOKEN", t.to_string()))
        .into_iter()
        .chain(args.offline.then(|| ("ARNIS_OFFLINE", "1".to_string())))
        .chain(
            (!args.overpass_url.is_empty())
                .then(|| ("ARNIS_OVERPASS_URL", args.overpass_url.join(","))),
        )
        .collect();
    // The Overture release listing is asked once for the pieces, only when one is left to
    // build and reads Overture, and in the background: a piece started before the answer
    // asks for itself.
    let releases = Arc::new(OnceLock::new());
    if args.overture && !args.skip_objects() && (!queue.is_empty() || head.is_some()) {
        let releases = Arc::clone(&releases);
        std::thread::spawn(move || releases.set(crate::overture::shared_releases_env()));
    }
    let piece_env = || -> Vec<(&str, String)> {
        let listing = releases.get().cloned().flatten();
        token.iter().cloned().chain(listing).collect()
    };
    let lease_for = |i: usize| -> Result<UnitLease, String> {
        Ok(UnitLease {
            piece: i as u32,
            area_id: job.area_id(&units[i]),
            extending: head != Some(i),
            first_map_id: job.first_map_id(world_dir, i)?,
            rect: corners(&units[i].rect),
            job: corners(&rect),
            tiled: crate::tile::create_tiles(&rect, crate::tile::DEFAULT_TILE_SIZE).len() >= 3,
        })
    };
    // Writes piece `i`'s lease; returns it with the bbox the piece builds, halo included.
    let piece_child = |i: usize| -> Result<(PathBuf, String), String> {
        let lease = job.lease_path(i);
        write(
            &lease,
            &serde_json::to_value(lease_for(i)?).map_err(|e| e.to_string())?,
        )?;
        let built = one_world::piece_build_rect(&units[i].rect, &rect);
        let bbox = crate::projection::llbbox_for_rect(&proj, &built)?.arg();
        Ok((lease, bbox))
    };

    // `--prewarm-first`: one piece at a time, each with the job's whole download allowance, so N
    // workers then read the caches instead of all fetching at once. `--prewarm` stops after it.
    // The warm step fills the start of the job's bar, the build the rest.
    let mut warm_share = 0.0;
    if args.prewarm || args.prewarm_first {
        // Only the pieces this run builds: not those built, left out or skipped as built.
        let warm: Vec<usize> = head.into_iter().chain(queue.iter().copied()).collect();
        if !warm.is_empty() {
            warm_share = if args.prewarm { 100.0 } else { WARM_SHARE };
        }
        // A lease already there marks a piece an earlier run started; `--prewarm` must keep it.
        let fresh: Vec<usize> = warm
            .iter()
            .copied()
            .filter(|&i| !job.lease_path(i).exists())
            .collect();
        let stopped = || STOP.load(Ordering::Acquire);
        for (k, &i) in warm.iter().enumerate() {
            if !wait_while_paused() {
                return Err(STOPPED.to_string());
            }
            // Warm what the piece will build: its rect plus the halo.
            let (lease, bbox) = piece_child(i)?;
            // The job's bar: the pieces warmed so far plus this one's share.
            let report = |f: f64| {
                let pct = (k as f64 + f) / warm.len() as f64 * warm_share;
                crate::progress::emit_gui_progress_update(pct, "");
            };
            // Before a build, warming only spares it downloads: none are left to spare.
            let cached = crate::coordinate_system::geographic::LLBBox::from_str(&bbox)
                .is_ok_and(|area| inputs_cached(args, area));
            if cached && !args.prewarm {
                println!("  piece {}/{of}: already cached", i + 1);
                report(1.0);
                continue;
            }
            let mut warm_argv = child_args(argv.iter().cloned(), &bbox, &lease, false);
            let downloads = args
                .max_downloads
                .unwrap_or(crate::net::MAX_CONCURRENT_REQUESTS as u32)
                .to_string();
            warm_argv.extend([
                "--prewarm".into(),
                "--max-downloads".into(),
                downloads.into(),
            ]);
            println!("  piece {}/{of}: warming the caches", i + 1);
            let log = piece_log_path(&job.dir, i);
            let env = piece_env();
            if let Err(f) = run_piece_until(&warm_argv, &env, &stopped, Path::new(&log), report) {
                if STOP.load(Ordering::Acquire) {
                    return Err(STOPPED.to_string());
                }
                if args.prewarm {
                    return Err(format!("warming piece {} failed: {}", i + 1, f.message));
                }
                // The build fetches what is missing; the warm step lists each as "  - what".
                let missed: Vec<&str> = f
                    .message
                    .lines()
                    .filter_map(|l| l.strip_prefix("  - "))
                    .collect();
                let what = if missed.is_empty() {
                    format!("see {log}")
                } else {
                    missed.join(", ")
                };
                eprintln!(
                    "Warning: piece {} could not cache everything ({what}); it downloads the rest \
                     while it builds.",
                    i + 1
                );
            }
        }
        if args.prewarm {
            // Only what it wrote: a job already under way keeps its resume records.
            if job_existed {
                for &i in &fresh {
                    let _ = std::fs::remove_file(job.lease_path(i));
                }
                let _ = std::fs::remove_dir(&job.dir);
            } else {
                let _ = std::fs::remove_dir_all(&job.dir);
            }
            let _ = std::fs::remove_dir(world_dir.join(one_world::JOBS_DIR));
            return Ok(());
        }
    }

    // Each piece's fraction done, and the highest job percentage sent.
    let done: Mutex<(Vec<f64>, f64)> = Mutex::new((
        lock(&results)
            .iter()
            .map(|r| if r.is_some() { 1.0 } else { 0.0 })
            .collect(),
        0.0,
    ));
    let finished = AtomicUsize::new(lock(&results).iter().flatten().count());
    // How far the job is, kept in the manifest for the window's Projects list.
    // The command line has no window settings; it keeps the ones a window run recorded.
    let settings = Some(settings).filter(|s| !s.is_empty());
    let first = Some((settings, asked));
    let record_job = |start: Option<(Option<&[String]>, [f64; 4])>| {
        let _guard = lock(&MANIFEST);
        let done = finished.load(Ordering::Relaxed);
        if let Err(e) = one_world::record_job(
            world_dir,
            &job.name,
            start.and_then(|s| s.0),
            start.map(|s| s.1),
            done as u32,
            of as u32,
            done == of,
        ) {
            eprintln!("Warning: Failed to record the job's progress: {e}");
        }
    };
    record_job(first);
    let status = || {
        if !crate::progress::is_running_with_gui() {
            // `--progress json` has its piece records, so its stream stays as it was.
            return String::new();
        }
        let line = format!(
            "Building pieces... {}/{of} done",
            finished.load(Ordering::Relaxed)
        );
        if PAUSE.load(Ordering::Acquire) {
            return line + ". Paused.";
        }
        line
    };
    // Piece `i` is `f` done: moves the job's bar.
    let report = |i: usize, f: f64| {
        let mut d = lock(&done);
        d.0[i] = f;
        // A retried piece starts over; the bar waits for it instead.
        d.1 = d.1.max(job_percent(&d.0));
        let percent = warm_share + (100.0 - warm_share) * d.1 / 100.0;
        drop(d);
        crate::progress::emit_gui_progress_update(percent, &status());
    };
    let cap = AtomicUsize::new(sizing.workers);
    // `--stitch-map`: the head piece draws the map item from itself alone; once all are built
    // it is redrawn from every piece's preview (a One World area always writes one).
    let stitch_map =
        args.units.stitch_map && args.map_item && !args.bedrock && !args.luanti && of > 1;
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let aborting = AtomicBool::new(false);
    // The cells left to build, which the window may hold, reorder, cancel and retry.
    let queue = Arc::new(Mutex::new(Queue {
        pending: queue,
        skipped: (0..of)
            .filter(|&i| job.skip.contains(&i) && lock(&results)[i].is_none())
            .collect(),
        ..Default::default()
    }));
    let _active = Active::register(world_dir, &queue);

    // Builds piece `i` on worker slot `w`: Ok(None) when Stop or a cancel ended it or it
    // failed.
    let build = |i: usize, w: usize| -> Result<Option<PieceResult>, String> {
        let unit = &units[i];
        // The piece builds its halo too; its lease says which part it writes.
        let (lease, bbox) = piece_child(i)?;
        let argv = sizing.child_args(child_args(
            argv.iter().cloned(),
            &bbox,
            &lease,
            spawn_piece == Some(i),
        ));
        let child_env: Vec<(&str, String)> =
            sizing.child_env().into_iter().chain(piece_env()).collect();
        let end = |how: Outcome| lock(&queue).finish(i, how);
        let killed = || STOP.load(Ordering::Acquire) || lock(&queue).cancelled(i);
        let mut attempt = 0;
        let r = loop {
            println!(
                "  piece {}/{of}: {} chunks at x {} z {}{}",
                i + 1,
                unit.chunks(),
                unit.rect.min_x(),
                unit.rect.min_z(),
                if attempt > 0 { " (retry)" } else { "" }
            );
            let slot = w + 1;
            let state = if attempt > 0 { "retry" } else { "start" };
            piece_record(i, state, json!({"worker": slot, "pct": 0}));
            // The window hears of a piece's progress once a second, or at 5 % more.
            let mut sent = (std::time::Instant::now(), 0.0f64);
            let mut parallel = false;
            let seen = |f: f64| {
                report(i, f);
                if !parallel && f >= PARALLEL_FROM {
                    parallel = true;
                    lock(&queue).prologue.remove(&i);
                }
                let pct = f * 100.0;
                if sent.0.elapsed().as_secs() >= 1 || pct - sent.1 >= 5.0 {
                    sent = (std::time::Instant::now(), pct);
                    piece_progress(i, state, slot, pct);
                }
            };
            match run_piece_until(
                &argv,
                &child_env,
                &killed,
                Path::new(&piece_log_path(&job.dir, i)),
                seen,
            ) {
                Ok(r) => break r,
                // Killed by Stop: not a failure, and built again on resume.
                Err(_) if STOP.load(Ordering::Acquire) => {
                    end(Outcome::Stopped);
                    piece_record(i, "stopped", json!({"worker": slot}));
                    return Ok(None);
                }
                // Cancelled by the window: back in the queue, not a failure.
                Err(_) if lock(&queue).cancelled(i) => {
                    let state = end(Outcome::Cancelled).unwrap_or("queued");
                    piece_record(i, state, json!({"worker": slot}));
                    return Ok(None);
                }
                // Never once the job is aborting: a piece killed with it looks like a crash.
                Err(f)
                    if f.transient
                        && attempt < MAX_RETRIES
                        && !aborting.load(Ordering::Relaxed) =>
                {
                    attempt += 1;
                    eprintln!("Piece {} failed, retrying: {}", i + 1, f.message);
                    // Others may start while it waits; then it fetches again.
                    lock(&queue).prologue.remove(&i);
                    std::thread::sleep(std::time::Duration::from_secs(5 * attempt as u64));
                    lock(&queue).prologue.insert(i);
                }
                // The head piece makes the world's extras, so the job cannot go on without it.
                Err(f) if head == Some(i) => {
                    end(Outcome::Failed);
                    piece_record(i, "failed", json!({"worker": slot, "error": f.message}));
                    aborting.store(true, Ordering::Relaxed);
                    return Err(format!("piece {} of {of} failed: {}", i + 1, f.message));
                }
                Err(f) => {
                    end(Outcome::Failed);
                    piece_record(i, "failed", json!({"worker": slot, "error": f.message}));
                    eprintln!("Piece {} of {of} failed: {}", i + 1, f.message);
                    return Ok(None);
                }
            }
        };
        // Recorded here, not by the piece, so the manifest has one writer.
        {
            let _guard = lock(&MANIFEST);
            let area_id = job.area_id(unit);
            let run = RunContext {
                area_id,
                piece: Some(PieceRef {
                    job: job.name.clone(),
                    index: i as u32,
                }),
                ..ctx.clone()
            };
            let preview = run.preview_path();
            one_world::record_area(
                &run,
                &unit.bbox,
                &unit.rect,
                None,
                preview.is_file().then_some(preview.as_path()),
            )
            .map_err(|e| format!("The area was written but could not be recorded: {e}"))?;
            // The head piece made the world's map item, the one `--stitch-map` repaints.
            if head == Some(i) && args.map_item {
                let recorded = job
                    .kept_map_id(i)
                    .ok_or_else(|| "the piece's lease has no map id".to_string())
                    .and_then(|id| one_world::record_map_item(world_dir, id));
                if let Err(e) = recorded {
                    eprintln!("Warning: Failed to record the world map item: {e}");
                }
            }
        }
        write(&job.done_path(unit), &json!(r))?;
        // The world has its extras: a resume owes no head.
        if head == Some(i) {
            if let Err(e) = save_state(&job.dir, "head", Value::Null) {
                eprintln!("Warning: Failed to keep the job's state: {e}");
            }
        }
        let pieces_done = finished.fetch_add(1, Ordering::Relaxed) + 1;
        record_job(None);
        piece_record(
            i,
            "done",
            json!({"peak_rss_mb": r.peak_rss_mb, "wall_s": r.wall_s, "pieces_done": pieces_done,
                   "worker": w + 1, "pct": 100}),
        );
        progress_json::CHUNKS_WRITTEN.fetch_add(r.chunks, Ordering::Relaxed);
        report(i, 1.0);
        lock(&results)[i] = Some(r.clone());
        end(Outcome::Built);
        Ok(Some(r))
    };

    // The head piece goes first, on worker 0; like any piece, it holds the others back
    // until it builds in parallel.
    if let Some(i) = head {
        let mut q = lock(&queue);
        q.running.insert(i);
        q.prologue.insert(i);
    }
    let work = |w: usize| -> Result<(), String> {
        // Pause holds the head too; Stop while it waits ends the job below.
        if let (0, Some(i)) = (w, head) {
            if wait_while_paused() {
                build(i, 0)?;
            }
        }
        while !aborting.load(Ordering::Relaxed) && !STOP.load(Ordering::Acquire) {
            let next = {
                let mut q = lock(&queue);
                // Paused: the queue waits while the running pieces finish.
                let start = !PAUSE.load(Ordering::Acquire) && q.admits(cap.load(Ordering::Relaxed));
                q.next(start)
            };
            let i = match next {
                Next::Cell(i) => i,
                Next::Wait => {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                }
                Next::Done => break,
            };
            let built = build(i, w);
            // `auto` lowers the cap once a finished piece shows pieces need more memory.
            if let Ok(Some(PieceResult {
                peak_rss_mb: Some(peak),
                ..
            })) = &built
            {
                let allowed = sizing.cap_for(*peak);
                if cap.fetch_min(allowed, Ordering::Relaxed) > allowed {
                    println!("  pieces peak at {peak} MB: {allowed} at a time from now on");
                }
            }
            built?;
        }
        Ok(())
    };
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..sizing.workers)
            .map(|w| {
                let (work, aborting, failure) = (&work, &aborting, &failure);
                s.spawn(move || {
                    if let Err(e) = work(w) {
                        aborting.store(true, Ordering::Relaxed);
                        lock(failure).get_or_insert(e);
                    }
                })
            })
            .collect();
        // What the window changed (held, back in the queue) goes out here, in full records.
        loop {
            let over = workers.iter().all(|t| t.is_finished());
            let (events, skip) = {
                let mut q = lock(&queue);
                let skip = std::mem::take(&mut q.skip_changed).then(|| q.skipped.clone());
                (std::mem::take(&mut q.events), skip)
            };
            for (i, state) in events {
                let reason = (state == "skipped").then_some("skip_cells");
                piece_record(i, state, json!({ "reason": reason }));
            }
            if let Err(e) = skip.map_or(Ok(()), |skip| save_skip(&job.dir, &skip)) {
                eprintln!("Warning: Failed to keep the cells left out: {e}");
            }
            if over {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
    lock(&queue).closed = true;
    record_job(None);
    if let Some(e) = failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Err(format!("{e}\n{RESUME}"));
    }
    let failed: Vec<usize> = lock(&queue).failed.iter().copied().collect();
    if !failed.is_empty() && !STOP.load(Ordering::Acquire) {
        return Err(failed_summary(failed, of));
    }
    // Left out: the job is built without them and keeps its state for a later run.
    let skipped = lock(&queue).skipped.len();
    // Stopped before the last piece, or only some retried: kept for a resume.
    if finished.load(Ordering::Relaxed) + skipped < of {
        let partial = only.is_some() && !STOP.load(Ordering::Acquire);
        return Err(if partial { PARTIAL } else { STOPPED }.to_string());
    }

    // The pieces' map ids were their own; the counter now covers all of them.
    if let Err(e) = crate::map_item::sync_map_counter(world_dir) {
        eprintln!("Warning: Failed to update the map id counter: {e}");
    }
    if stitch_map {
        match crate::map_item::repaint_job_map(world_dir) {
            Ok(Some(id)) => println!("World map item (map #{id}) redrawn over every piece."),
            Ok(None) => {}
            Err(e) => eprintln!("Warning: Failed to redraw the map item over every piece: {e}"),
        }
    }
    if let Err(e) = crate::world_utils::touch_last_played(world_dir) {
        eprintln!("Warning: Failed to update LastPlayed: {e}");
    }
    if args.world_border {
        one_world::apply_world_border(world_dir, &rect);
    }
    if skipped > 0 {
        println!("  {skipped} cell(s) left out; the job keeps them for a later run");
        return Ok(());
    }
    // The job is built; its state is not needed any more.
    let _ = std::fs::remove_dir_all(&job.dir);
    let _ = std::fs::remove_dir(world_dir.join(one_world::JOBS_DIR));
    Ok(())
}

/// Whether the caches hold every input of `area` that is counted on disk: the OSM, elevation,
/// land cover and Overture tiles. What is not counted, a build fetches.
fn inputs_cached(args: &Args, area: crate::coordinate_system::geographic::LLBBox) -> bool {
    crate::elevation::cache::user_cache_dir()
        .is_some_and(|root| all_cached(&crate::data_plan::plan(&root, args, area).items))
}

fn all_cached(items: &[crate::data_plan::Item]) -> bool {
    !items.is_empty() && items.iter().all(|i| i.total > 0 && i.cached == i.total)
}

/// Where piece `i`'s output is kept (absolute), in the job folder.
fn piece_log_path(job_dir: &Path, i: usize) -> String {
    let dir = std::path::absolute(job_dir).unwrap_or_else(|_| job_dir.to_path_buf());
    dir.join(format!("piece-{}.log", i + 1))
        .to_string_lossy()
        .into_owned()
}

/// What a job that kept going past failed pieces ends with: which, and how to retry them.
fn failed_summary(mut failed: Vec<usize>, of: usize) -> String {
    failed.sort_unstable();
    let numbers: Vec<String> = failed.iter().map(|i| (i + 1).to_string()).collect();
    format!(
        "{} of {of} pieces failed: {}. The others are built; retry the failed ones with \
         --only-pieces {}, or run the same command again.",
        failed.len(),
        numbers.join(", "),
        numbers.join(",")
    )
}

/// The job's area as the manifest keeps it, `[south, west, north, east]`: inside the outer
/// chunks and at the map's six decimals, so the window's Resume snaps it to `requested` again.
fn kept_bbox(proj: &crate::projection::WebMercatorProjection, requested: &XZBBox) -> [f64; 4] {
    const INSET: f64 = 4.0;
    let six = |v: f64| (v * 1e6).round() / 1e6;
    [
        six(proj.lat_for_z(requested.max_z() as f64 + 1.0 - INSET)),
        six(proj.lon_for_x(requested.min_x() as f64 + INSET)),
        six(proj.lat_for_z(requested.min_z() as f64 + INSET)),
        six(proj.lon_for_x(requested.max_x() as f64 + 1.0 - INSET)),
    ]
}

/// `[min_x, min_z, max_x, max_z]`.
fn corners(r: &XZBBox) -> [i32; 4] {
    [r.min_x(), r.min_z(), r.max_x(), r.max_z()]
}

/// The piece's area on the map: `[min_lat, min_lng, max_lat, max_lng]`.
fn bounds(u: &WorkUnit) -> [f64; 4] {
    let (lo, hi) = (u.bbox.min(), u.bbox.max());
    [lo.lat(), lo.lng(), hi.lat(), hi.lng()]
}

fn contains(r: &XZBBox, x: i32, z: i32) -> bool {
    r.contains(&crate::coordinate_system::cartesian::XZPoint::new(x, z))
}

fn spawn_piece(units: &[WorkUnit], spawn: Option<(i32, i32)>) -> Option<usize> {
    let (x, z) = spawn?;
    units.iter().position(|u| contains(&u.rect, x, z))
}

/// Where in `queue` the head piece waits: the one an earlier run still owes, else the one
/// holding the spawn, else the one nearest the centre. None when the world has its extras.
fn head_position(
    queue: &VecDeque<usize>,
    owed: Option<usize>,
    spawn: Option<usize>,
    fresh: bool,
) -> Option<usize> {
    if !fresh && owed.is_none() {
        return None;
    }
    let at = |piece: Option<usize>| queue.iter().position(|&i| Some(i) == piece);
    at(owed)
        .or_else(|| at(spawn))
        .or_else(|| (0..queue.len()).min_by_key(|&p| queue[p]))
}

const RESUME: &str = "Finished pieces are kept; run the same command again to resume.";

/// `--spawn-lat/--spawn-lng` in the world's blocks, as a run converts them.
fn user_spawn_xz(
    args: &Args,
    selection: &crate::coordinate_system::geographic::LLBBox,
) -> Option<(i32, i32)> {
    let point =
        crate::coordinate_system::geographic::LLPoint::new(args.spawn_lat?, args.spawn_lng?)
            .ok()?;
    let (transformer, _) = crate::projection::ProjectionSpec::from_args(args)
        .transformer(selection)
        .ok()?;
    let p = transformer.transform_point(point);
    Some((p.x, p.z))
}

/// Options the coordinator decides per piece, so the user's are dropped.
const PER_PIECE: &[&str] = &[
    "--piece-order",
    "--plan-mask",
    "--bbox",
    "--unit-regions",
    "--one-world-workers",
    "--plan-units",
    "--progress",
    "--threads",
    "--ram-budget-mb",
    "--max-downloads",
    "--one-world-unit",
    "--only-pieces",
    "--skip-cells",
    "--snap-regions",
    "--margin-regions",
    // Handed over in the environment, out of sight of other processes.
    "--mapillary-token",
];

/// Switches the coordinator decides: it hands `--prewarm` to the pieces it warms, and clap
/// refuses one given twice. They take no value, so the next argument is kept.
const PER_PIECE_SWITCHES: &[&str] = &[
    "--prewarm",
    "--prewarm-first",
    "--skip-built",
    "--stitch-map",
];

/// Only the piece the world spawns in gets the spawn point: the others do not contain it,
/// and a run refuses a spawn outside its area.
const SPAWN: &[&str] = &["--spawn-lat", "--spawn-lng"];

/// Options that leave the world as it is, so a resume may change them freely.
const MACHINE: &[&str] = &["--output-dir", "--path", "--world-name", "--downloader"];
const MACHINE_SWITCHES: &[&str] = &[
    "--debug",
    "--benchmark",
    "--offline",
    "--no-update-check",
    "--no-cache-sweep",
];

/// What in a job's command line decides its world: each option as `--name=value`, sorted.
fn generation_options(argv: &[OsString]) -> Vec<String> {
    let names = [PER_PIECE, MACHINE].concat();
    let switches = [PER_PIECE_SWITCHES, MACHINE_SWITCHES].concat();
    let mut out: Vec<String> = Vec::new();
    for arg in without_options(argv.to_vec(), &names, &switches) {
        let arg = arg.to_string_lossy().into_owned();
        match out.last_mut() {
            Some(last) if !arg.starts_with("--") => {
                last.push(if last.contains('=') { ',' } else { '=' });
                last.push_str(&arg);
            }
            _ => out.push(arg),
        }
    }
    let mut out: Vec<String> = out.iter().filter_map(|o| kept_option(o)).collect();
    out.sort();
    out
}

/// Options whose value is a path on this machine. A job does not keep them: a world may be shared.
const PATH_OPTIONS: &[&str] = &[
    "--file",
    "--polygon-file",
    "--osm-pbf",
    "--osm-tiles-url",
    "--plan-mask",
    "--tree-pack-dir",
    "--climate-map",
    "--cave-asset-pack",
    "--cave-zone-map",
    "--loot-table",
    "--building-facades-dir",
    "--mapillary-facades-dir",
    "--mapillary-debug-dir",
    "--mapillary-facade-debug-dir",
];

/// Options whose value is a server, which a job keeps without user name, password or query.
const URL_OPTIONS: &[&str] = &["--overpass-url", "--osm-pbf-url"];

/// `--name` or `--name=value` as a job keeps it: none for a path, a server without its
/// credentials or query.
pub(crate) fn kept_option(opt: &str) -> Option<String> {
    let (name, value) = opt.split_once('=').unwrap_or((opt, ""));
    if PATH_OPTIONS.contains(&name) {
        return None;
    }
    if !URL_OPTIONS.contains(&name) || value.is_empty() {
        return Some(opt.to_string());
    }
    let urls: Vec<String> = value.split(',').map(bare_url).collect();
    Some(format!("{name}={}", urls.join(",")))
}

/// `url` without user name, password, query or fragment.
fn bare_url(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or_default();
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if scheme.is_empty() {
        format!("{host}{path}")
    } else {
        format!("{scheme}://{host}{path}")
    }
}

/// Options the One World keeps from its first area (`one_world::prepare`), whatever a resume says.
const PINNED: &[&str] = &[
    "--seed",
    "--cave-seed",
    "--cave-datum-y",
    "--height-multiplier",
    "--disable-height-limit",
    "--aws-only-elevation",
    "--origin",
    "--projection",
];

/// The status line for a resume that `changed` these options; none when nothing changed.
fn settings_changed_notice(changed: &[String]) -> Option<String> {
    (!changed.is_empty()).then(|| format!("{SETTINGS_CHANGED} {}", changed.join(", ")))
}

/// The names of the options `a` and `b` disagree on, other than pinned ones and the spawn.
fn changed_options(a: &[String], b: &[String]) -> Vec<String> {
    let only = |x: &[String], y: &[String]| -> Vec<String> {
        x.iter()
            .filter(|o| !y.contains(o))
            .map(|o| o.split('=').next().unwrap_or_default().to_string())
            .filter(|name| !PINNED.contains(&name.as_str()) && !SPAWN.contains(&name.as_str()))
            .collect()
    };
    let mut names = [only(a, b), only(b, a)].concat();
    names.sort();
    names.dedup();
    names
}

/// `argv` without the options `names` and their values, and without the `switches`.
fn without_options(argv: Vec<OsString>, names: &[&str], switches: &[&str]) -> Vec<OsString> {
    let mut skip_value = false;
    argv.into_iter()
        .filter(|arg| {
            if std::mem::take(&mut skip_value) {
                return false;
            }
            let text = arg.to_string_lossy();
            let name = text.split('=').next().unwrap_or_default();
            let named = names.contains(&name);
            skip_value = named && !text.contains('=');
            !named && !switches.contains(&name)
        })
        .collect()
}

/// A piece's command line: the job's own, minus what is decided per piece, plus the
/// piece's bbox at full precision and its lease.
pub(crate) fn child_args(
    argv: impl Iterator<Item = OsString>,
    bbox: &str,
    lease: &Path,
    keep_spawn: bool,
) -> Vec<OsString> {
    let mut names = PER_PIECE.to_vec();
    if !keep_spawn {
        names.extend(SPAWN);
    }
    let mut out = without_options(argv.collect(), &names, PER_PIECE_SWITCHES);
    out.extend([
        "--bbox".into(),
        bbox.into(),
        "--one-world-unit".into(),
        lease.as_os_str().to_owned(),
        "--progress".into(),
        "json".into(),
    ]);
    out
}

/// Why a piece failed, and whether running it again may help.
struct PieceFailure {
    message: String,
    transient: bool,
}

/// Only a failed download, as the piece's own error record says, may pass on another try.
/// A crash (a panic, a signal, a Windows exception code) is not retried.
fn is_transient(code: Option<i32>, kind: Option<&str>) -> bool {
    code == Some(1) && kind == Some(progress_json::NETWORK)
}

/// A piece's log, appended to so a retry (or the build after a prewarm) keeps the
/// first failure; each build try opens with a `--- try N ---` line, the warm step with
/// `--- warm ---`, which is not a try.
fn open_piece_log(path: &Path, warm: bool) -> Option<std::fs::File> {
    let tries = std::fs::read_to_string(path)
        .map(|t| t.lines().filter(|l| l.starts_with("--- try ")).count())
        .unwrap_or(0);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    if warm {
        writeln!(f, "--- warm ---").ok()?;
    } else {
        writeln!(f, "--- try {} ---", tries + 1).ok()?;
    }
    Some(f)
}

/// Variables a run reads that a piece takes from the coordinator only: the window ignores
/// them in its own environment.
const PIECE_ENV: &[&str] = &["ARNIS_OFFLINE", "ARNIS_OVERPASS_URL", "MAPILLARY_TOKEN"];

/// A piece run of `exe` with `argv`, given of [`PIECE_ENV`] only what `env` sets.
fn piece_command(exe: &Path, argv: &[OsString], env: &[(&str, String)]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(argv);
    for name in PIECE_ENV {
        cmd.env_remove(name);
    }
    cmd.envs(env.iter().map(|(k, v)| (k, v)));
    cmd
}

/// Runs one piece to the end, or kills it as soon as `cancel` says so. `progress` gets the
/// piece's own fraction done. Its output lines are also written to `log`.
fn run_piece_until(
    argv: &[OsString],
    env: &[(&str, String)],
    cancel: &(dyn Fn() -> bool + Sync),
    log: &Path,
    mut progress: impl FnMut(f64),
) -> Result<PieceResult, PieceFailure> {
    let fail = |message: String| PieceFailure {
        message,
        transient: false,
    };
    let exe =
        std::env::current_exe().map_err(|e| fail(format!("Cannot find this executable: {e}")))?;
    let mut cmd = piece_command(&exe, argv, env);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    child::prepare(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| fail(format!("Failed to start a piece: {e}")))?;
    if let Err(e) = child::adopt(&child) {
        let _ = child.kill();
        return Err(fail(format!("Could not tie a piece to this process: {e}")));
    }
    // Held open for the piece's life: on Unix it exits when this closes.
    let _stdin = child.stdin.take();
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL_LINES)));
    // Only the warm step's command line carries `--prewarm`: the coordinator strips it.
    let warm = argv.iter().any(|a| a == "--prewarm");
    let log = Arc::new(Mutex::new(open_piece_log(log, warm)));
    let push =
        |tail: &Mutex<VecDeque<String>>, log: &Mutex<Option<std::fs::File>>, line: String| {
            if let Some(f) = lock(log).as_mut() {
                let _ = writeln!(f, "{line}");
            }
            let mut t = lock(tail);
            if t.len() == TAIL_LINES {
                t.pop_front();
            }
            t.push_back(line);
        };
    let stderr = child.stderr.take().map(|err| {
        let (tail, log) = (Arc::clone(&tail), Arc::clone(&log));
        std::thread::spawn(move || {
            for line in BufReader::new(err).split(b'\n').map_while(Result::ok) {
                push(
                    &tail,
                    &log,
                    String::from_utf8_lossy(&line).trim_end().to_string(),
                );
            }
        })
    });
    let mut result = PieceResult::default();
    let mut kind = None;
    let stdout = child.stdout.take();
    let child = Mutex::new(child);
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Killing the child closes its pipes, which ends the read below.
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancel() {
                    let _ = lock(&child).kill();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        let Some(out) = stdout else {
            finished.store(true, Ordering::Release);
            return;
        };
        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line).trim_end().to_string();
            let Some(record) = piece_record_of(&line) else {
                push(&tail, &log, line);
                continue;
            };
            match record["type"].as_str() {
                Some("phase") | Some("progress") => {
                    if let Some(p) = record["progress"].as_f64() {
                        progress(p / 100.0);
                    }
                    if let Some(notice) = forwarded_notice(&record) {
                        crate::progress::emit_gui_progress_update(
                            crate::progress::MESSAGE_ONLY,
                            notice,
                        );
                    }
                }
                Some("done") => {
                    result.peak_rss_mb = record["peak_rss_mb"].as_u64();
                    result.wall_s = record["wall_s"].as_f64();
                    result.chunks = record["chunks"].as_u64().unwrap_or(0);
                }
                Some("error") => {
                    kind = record["kind"].as_str().map(str::to_owned);
                    push(&tail, &log, line)
                }
                _ => {}
            }
        }
        finished.store(true, Ordering::Release);
    });
    let status = lock(&child).wait().map_err(|e| fail(e.to_string()))?;
    if let Some(t) = stderr {
        let _ = t.join();
    }
    if status.success() {
        return Ok(result);
    }
    let tail: Vec<String> = lock(&tail).iter().cloned().collect();
    Err(PieceFailure {
        transient: is_transient(status.code(), kind.as_deref()),
        message: format!("{status}; last output:\n{}", tail.join("\n")),
    })
}

/// A `--progress json` record in a piece's output line; `None` for plain output.
fn piece_record_of(line: &str) -> Option<Value> {
    line.starts_with(r#"{"v":1"#)
        .then(|| serde_json::from_str(line).ok())
        .flatten()
}

/// A piece's `phase` the job passes on as its own: a Region Download fallback notice.
fn forwarded_notice(record: &Value) -> Option<&str> {
    let name = record["name"].as_str()?;
    (record["type"] == "phase" && name.starts_with(crate::retrieve_data::REGION_DOWNLOAD_FALLBACK))
        .then_some(name)
}

/// The job's percentage: every piece is an equal share, so it reads n/N as "n/N done"
/// does, plus the running pieces' own fractions.
fn job_percent(done: &[f64]) -> f64 {
    done.iter().map(|f| f.clamp(0.0, 1.0)).sum::<f64>() / done.len().max(1) as f64 * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;

    #[test]
    fn a_piece_log_keeps_every_try() {
        let p = std::env::temp_dir().join(format!("piece-log-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&p);
        for (warm, line) in [
            (true, "warmed"),
            (false, "first failure"),
            (false, "second"),
        ] {
            writeln!(open_piece_log(&p, warm).unwrap(), "{line}").unwrap();
        }
        let t = std::fs::read_to_string(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(
            t,
            "--- warm ---
warmed
--- try 1 ---
first failure
--- try 2 ---
second
"
        );
    }

    #[test]
    fn the_queue_respects_held_front_cancel_and_retry() {
        let mut q = Queue {
            pending: VecDeque::from([0, 1, 2, 3, 4]),
            ..Default::default()
        };
        // Held cells wait; the rest go on in order.
        assert_eq!(q.apply("hold", &[1, 2, 9]).unwrap(), [1, 2]);
        assert_eq!(q.events, [(1, "held"), (2, "held")]);
        assert_eq!(q.next(true), Next::Cell(0));
        assert_eq!(q.next(true), Next::Cell(3));
        // Front, in the order given; held ones move too but still wait.
        assert_eq!(q.apply("front", &[4, 1, 0]).unwrap(), [4, 1]);
        assert_eq!(q.pending, [4, 1, 2]);
        assert_eq!(q.next(false), Next::Wait);
        assert_eq!(q.next(true), Next::Cell(4));
        // Only held pending cells left: the job waits for them.
        assert_eq!(q.next(true), Next::Wait);
        // Cancel kills a running cell, which goes back to pending; pending ones are ignored.
        assert_eq!(q.apply("cancel", &[3, 1]).unwrap(), [3]);
        assert!(q.cancelled(3) && !q.cancelled(0));
        assert_eq!(q.finish(3, Outcome::Cancelled), Some("queued"));
        assert!(!q.cancelled(3) && q.pending.back() == Some(&3));
        // Resume: back in line where they were.
        assert_eq!(q.apply("resume", &[2, 1]).unwrap(), [2, 1]);
        assert_eq!(q.next(true), Next::Cell(1));
        assert_eq!(q.next(true), Next::Cell(2));
        assert_eq!(q.next(true), Next::Cell(3));
        // A failed cell is retried at the back; a held running cell that fails is not held.
        q.apply("hold", &[0]).unwrap();
        assert_eq!(q.finish(0, Outcome::Failed), None);
        assert!(q.held.is_empty() && q.failed.contains(&0));
        assert_eq!(q.apply("retry", &[0, 1]).unwrap(), [0]);
        for i in [1, 2, 3, 4] {
            q.finish(i, Outcome::Built);
        }
        assert_eq!(q.next(true), Next::Cell(0));
        assert_eq!(q.next(true), Next::Wait);
        q.finish(0, Outcome::Built);
        assert_eq!(q.next(true), Next::Done);
        assert!(q.apply("explode", &[0]).is_err());
    }

    #[test]
    fn pause_holds_until_resumed_and_stop_ends_the_wait() {
        assert!(wait_while_paused());
        PAUSE.store(true, Ordering::Release);
        let resume = std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            PAUSE.store(false, Ordering::Release);
        });
        let t = std::time::Instant::now();
        assert!(wait_while_paused() && t.elapsed().as_millis() >= 300);
        resume.join().unwrap();
        PAUSE.store(true, Ordering::Release);
        STOP.store(true, Ordering::Release);
        assert!(!wait_while_paused());
        PAUSE.store(false, Ordering::Release);
        STOP.store(false, Ordering::Release);
    }

    #[test]
    fn the_next_cell_waits_for_the_running_ones_to_build_in_parallel() {
        let mut q = Queue {
            pending: VecDeque::from([0, 1, 2]),
            ..Default::default()
        };
        assert!(q.admits(2));
        assert_eq!(q.next(true), Next::Cell(0));
        // Still fetching: nothing else starts, whatever the room.
        assert!(!q.admits(2));
        q.prologue.remove(&0);
        assert!(q.admits(2) && !q.admits(1));
        assert_eq!(q.next(true), Next::Cell(1));
        // A cell that ends in its prologue stops holding the queue.
        q.finish(1, Outcome::Failed);
        assert!(q.admits(2));
    }

    #[test]
    fn only_a_piece_with_every_counted_input_cached_skips_its_warming() {
        let item = |cached, total| crate::data_plan::Item {
            source: "osm",
            cached,
            cached_bytes: 0,
            total,
            missing_bytes: None,
        };
        assert!(all_cached(&[item(4, 4), item(169, 169)]));
        assert!(!all_cached(&[item(4, 4), item(168, 169)]));
        // A source that cannot be counted (Overpass) may still have to download.
        assert!(!all_cached(&[item(4, 4), item(0, 0)]));
        assert!(!all_cached(&[]));
    }

    #[test]
    fn the_head_piece_is_owed_until_it_is_built() {
        let queue = VecDeque::from([3, 1, 2]);
        // A fresh world: the spawn's piece, else the one nearest the centre.
        assert_eq!(head_position(&queue, None, Some(2), true), Some(2));
        assert_eq!(head_position(&queue, None, Some(9), true), Some(1));
        assert_eq!(head_position(&VecDeque::new(), None, None, true), None);
        // A world with areas has its extras, unless an earlier run's head never got built.
        assert_eq!(head_position(&queue, None, Some(2), false), None);
        assert_eq!(head_position(&queue, Some(3), Some(2), false), Some(0));
        // The owed piece is not built by this run: another one is the head.
        assert_eq!(head_position(&queue, Some(7), None, false), Some(1));
    }

    #[test]
    fn skipped_cells_leave_the_queue_and_come_back() {
        let mut q = Queue {
            pending: VecDeque::from([0, 1, 2]),
            running: BTreeSet::from([3]),
            ..Default::default()
        };
        // Only pending cells can be left out.
        assert_eq!(q.apply("skip", &[1, 3]).unwrap(), [1]);
        assert!(q.skip_changed && q.skipped == BTreeSet::from([1]));
        assert_eq!(q.events, [(1, "skipped")]);
        assert_eq!(q.next(true), Next::Cell(0));
        assert_eq!(q.next(true), Next::Cell(2));
        q.skip_changed = false;
        assert_eq!(q.apply("include", &[1, 2]).unwrap(), [1]);
        assert!(q.skip_changed && q.skipped.is_empty());
        assert_eq!(q.next(true), Next::Cell(1));
    }

    #[test]
    fn the_skip_set_round_trips_through_the_job_state() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(one_world::JOBS_DIR).join("j");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("job.json"), job_state(&units).to_string()).unwrap();
        let mut job = Job::open(tmp.path(), "j".into(), &units, &[], true).unwrap();
        assert!(job.skip.is_empty());
        job.save_skip(BTreeSet::from([1, 3])).unwrap();
        // A restart reads it back; the reserved ids stay.
        let job = Job::open(tmp.path(), "j".into(), &units, &[], true).unwrap();
        assert_eq!(job.skip, BTreeSet::from([1, 3]));
        assert_eq!(job.area_id(&units[2]), 7);
        // The head a run chose is kept beside it until it is built.
        assert_eq!(job.head, None);
        save_state(&dir, "head", json!(Some(2))).unwrap();
        let job = Job::open(tmp.path(), "j".into(), &units, &[], true).unwrap();
        assert_eq!((job.head, &job.skip), (Some(2), &BTreeSet::from([1, 3])));
        save_state(&dir, "head", Value::Null).unwrap();
        let job = Job::open(tmp.path(), "j".into(), &units, &[], true).unwrap();
        assert_eq!(job.head, None);
        let parse = |argv: &[&str]| <Args as clap::Parser>::try_parse_from(argv).unwrap();
        let plain = parse(&["arnis"]);
        assert_eq!(
            skipped_cells(tmp.path(), "j", &plain),
            BTreeSet::from([1, 3])
        );
        // The flag wins; an empty list leaves nothing out.
        let flag = parse(&["arnis", "--skip-cells", "2, 0"]);
        assert_eq!(
            skipped_cells(tmp.path(), "j", &flag),
            BTreeSet::from([0, 2])
        );
        let none = parse(&["arnis", "--skip-cells="]);
        assert!(skipped_cells(tmp.path(), "j", &none).is_empty());
        assert!(<Args as clap::Parser>::try_parse_from(["arnis", "--skip-cells", "x"]).is_err());
        // Only a job built in pieces takes it.
        let alone = parse(&["arnis", "--one-world", "--skip-cells", "1"]);
        let err = crate::args::validate_args(&alone).unwrap_err();
        assert!(err.contains("--skip-cells"), "{err}");
    }

    #[test]
    fn cell_actions_need_a_running_job() {
        assert!(control("hold", &[0]).unwrap_err().contains("No job"));
        let queue = Arc::new(Mutex::new(Queue {
            pending: VecDeque::from([0, 1]),
            ..Default::default()
        }));
        let tmp = tempfile::tempdir().unwrap();
        let active = Active::register(tmp.path(), &queue);
        let r = control("front", &[1, 7]).unwrap();
        assert_eq!(
            (r.applied, r.ignored, r.pending),
            (vec![1], vec![7], vec![1, 0])
        );
        // No cells come out of a world a job is building.
        let err = one_world::remove_cells(tmp.path(), "J", &[0]).unwrap_err();
        assert!(err.contains("job is building"), "{err}");
        // Once no worker takes a cell, an action would be lost: refused.
        lock(&queue).pending.clear();
        assert_eq!(lock(&queue).next(true), Next::Done);
        assert!(control("retry", &[0]).unwrap_err().contains("finishing"));
        drop(active);
        assert!(control("hold", &[0]).is_err());
    }

    #[test]
    fn live_with_workers_is_refused() {
        let parse = |argv: &[&str]| <Args as clap::Parser>::try_parse_from(argv).unwrap();
        for workers in ["4", "auto"] {
            let args = parse(&[
                "arnis",
                "--live",
                "--one-world",
                "--one-world-workers",
                workers,
            ]);
            let err = crate::args::validate_args(&args).unwrap_err();
            assert!(
                err.contains("--live") && err.contains("--one-world-workers"),
                "{err}"
            );
        }
        let one = parse(&["arnis", "--live", "--one-world", "--one-world-workers", "1"]);
        assert!(!crate::args::validate_args(&one).is_err_and(|e| e.contains("--live")));
    }

    #[test]
    fn piece_record_has_log_path() {
        let tmp = tempfile::tempdir().unwrap();
        let p = piece_log_path(tmp.path(), 2);
        assert!(
            Path::new(&p).is_absolute() && p.ends_with("piece-3.log"),
            "{p}"
        );
        // A piece's output lands in that file.
        let _ = run_piece_until(
            &["--no-such-flag".into()],
            &[],
            &|| false,
            Path::new(&p),
            |_| {},
        );
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("no-such-flag"));
    }

    #[test]
    fn a_job_keeps_no_paths_or_server_credentials() {
        let argv: Vec<OsString> = [
            "--scale=1",
            "--osm-pbf",
            "/home/me/x.pbf",
            "--file=/home/me/a.json",
            "--osm-tiles-url=/home/me/archive",
            "--overpass-url=https://me:pw@a.org/api?key=1,b.org/x#f",
            "--osm-pbf-url",
            "https://d.org/x.osm.pbf?token=2",
        ]
        .map(OsString::from)
        .to_vec();
        assert_eq!(
            generation_options(&argv),
            [
                "--osm-pbf-url=https://d.org/x.osm.pbf",
                "--overpass-url=https://a.org/api,b.org/x",
                "--scale=1",
            ]
        );
        assert_eq!(bare_url("me@host:8080"), "host:8080");
        assert_eq!(bare_url("http://h/p@q"), "http://h/p@q");
    }

    #[test]
    fn a_piece_takes_only_the_coordinators_settings_from_the_environment() {
        let cmd = piece_command(
            Path::new("arnis"),
            &[],
            &[("MAPILLARY_TOKEN", "MLY|x".into())],
        );
        let envs: Vec<_> = cmd.get_envs().collect();
        let name = std::ffi::OsStr::new;
        assert!(envs.contains(&(name("ARNIS_OFFLINE"), None)));
        assert!(envs.contains(&(name("ARNIS_OVERPASS_URL"), None)));
        assert!(envs.contains(&(name("MAPILLARY_TOKEN"), Some(name("MLY|x")))));
    }

    #[test]
    fn a_job_with_failed_pieces_names_them_for_a_retry() {
        let msg = failed_summary(vec![6, 2], 12);
        assert!(msg.starts_with("2 of 12 pieces failed: 3, 7."), "{msg}");
        assert!(msg.contains("--only-pieces 3,7,"), "{msg}");
        let args = <Args as clap::Parser>::try_parse_from(["arnis", "--only-pieces", "3,7"]);
        assert_eq!(args.unwrap().units.only_pieces, Some(vec![3, 7]));
    }

    /// A job's state as `Job::open` writes it: area ids from 5.
    fn job_state(units: &[WorkUnit]) -> Value {
        let slots: serde_json::Map<String, Value> = units
            .iter()
            .map(|u| (done_key(u), json!(u.index)))
            .collect();
        json!({"area_base": 5, "slots": slots})
    }

    #[test]
    fn job_folders_resume_by_region_key() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(one_world::JOBS_DIR).join("j");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("job.json"), job_state(&units).to_string()).unwrap();
        let job = Job::open(tmp.path(), "j".into(), &units, &[], true).unwrap();
        assert!(job.finished(&units[1]).is_none());
        assert_eq!(job.area_id(&units[2]), 7);
        // Pieces finish under their region key.
        let key = units[1].region_key();
        let (rx, rest) = key.split_once(',').unwrap();
        let (rz, s) = rest.split_once(',').unwrap();
        let want = dir.join(format!("done-r{rx}_{rz}_s{s}.json"));
        assert_eq!(job.done_path(&units[1]), want);
        std::fs::write(&want, r#"{"chunks":9}"#).unwrap();
        assert_eq!(job.finished(&units[1]).map(|r| r.chunks), Some(9));
        assert!(piece_done(tmp.path(), "j", &units[1]) && !piece_done(tmp.path(), "j", &units[0]));
        // `--skip-built`: a piece whose every chunk exists is skipped, unless this job
        // started it. Region (-1, -1) is unit 0's here, every chunk header set.
        let world = tmp.path();
        std::fs::create_dir_all(world.join("region")).unwrap();
        let u = units
            .iter()
            .find(|u| u.rect.min_x() >= -512 && u.rect.max_x() < 0 && u.rect.max_z() < 0)
            .unwrap();
        assert!(!job.built_before(world, u, &[]));
        std::fs::write(world.join("region/r.-1.-1.mca"), vec![1u8; 4096]).unwrap();
        assert!(job.built_before(world, u, &[]));
        // Another job started a piece over it and did not finish: maybe half written.
        let other = world.join(one_world::JOBS_DIR).join("k");
        std::fs::create_dir_all(&other).unwrap();
        let lease = UnitLease {
            piece: 0,
            area_id: 0,
            extending: true,
            first_map_id: 0,
            rect: [-600, -600, -100, -100],
            job: [-600, -600, -100, -100],
            tiled: false,
        };
        std::fs::write(other.join("piece-0.lease.json"), json!(lease).to_string()).unwrap();
        assert!(!job.built_before(world, u, &unfinished_pieces(world)));
        std::fs::write(other.join("done-r-2_-2_s1.json"), "{}").unwrap();
        assert!(job.built_before(world, u, &unfinished_pieces(world)));
        std::fs::write(job.lease_path(u.index as usize), "{}").unwrap();
        assert!(!job.built_before(world, u, &[]));
        // A piece's share follows its slot, not its plan index.
        let mut slots: serde_json::Map<String, Value> = units
            .iter()
            .map(|u| (done_key(u), json!(u.index)))
            .collect();
        slots.insert(done_key(&units[2]), json!(11));
        let state = json!({"area_base": 5, "slots": slots});
        std::fs::write(dir.join("job.json"), state.to_string()).unwrap();
        let job = Job::open(tmp.path(), "j".into(), &units, &[], true).unwrap();
        assert_eq!(job.area_id(&units[2]), 16);
        assert_eq!(job.area_id(&units[3]), 8);
        // Slots kept but one of this plan's pieces missing: another plan, refused.
        slots.remove(&done_key(&units[3]));
        let state = json!({"area_base": 5, "slots": slots});
        std::fs::write(dir.join("job.json"), state.to_string()).unwrap();
        let err = Job::open(tmp.path(), "j".into(), &units, &[], true)
            .err()
            .unwrap();
        assert!(err.contains("planned with other pieces"), "{err}");
    }

    #[test]
    fn cells_the_manifest_records_stay_built_once_the_job_folder_is_gone() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path().join("W");
        let mut args = <Args as clap::Parser>::parse_from(["arnis", "--bbox", &req.arg()]);
        drop(one_world::prepare(&w, &req, &mut args).unwrap());
        let ctx = args.one_world_run.clone().unwrap();
        for u in &units {
            let run = RunContext {
                area_id: 10 + u.index,
                piece: Some(PieceRef {
                    job: "j".into(),
                    index: u.index,
                }),
                ..ctx.clone()
            };
            one_world::record_area(&run, &u.bbox, &u.rect, None, None).unwrap();
        }
        let of = units.len() as u32;
        one_world::record_job(&w, "j", None, None, of, of, true).unwrap();
        // A finished job run again builds every cell, also once the run has marked it unfinished.
        assert!(built_cells(&w, "j").is_empty());
        let job = Job::open(&w, "j".into(), &units, &[], true).unwrap();
        one_world::record_job(&w, "j", None, None, 0, of, false).unwrap();
        assert!(built_cells(&w, "j").is_empty() && job.finished(&units[0]).is_none());
        let job = Job::open(&w, "j".into(), &units, &[], true).unwrap();
        assert!(job.finished(&units[0]).is_none());
        std::fs::remove_dir_all(&job.dir).unwrap();
        // One cell removed from the finished job: only that one is to build.
        one_world::record_job(&w, "j", None, None, of, of, true).unwrap();
        one_world::remove_cells(&w, "j", &[1]).unwrap();
        let job = Job::open(&w, "j".into(), &units, &[], true).unwrap();
        let built: Vec<u32> = units
            .iter()
            .filter(|u| job.finished(u).is_some())
            .map(|u| u.index)
            .collect();
        assert_eq!(built.len(), units.len() - 1);
        assert!(!built.contains(&1) && built_cells(&w, "j").len() == built.len());
    }

    #[test]
    fn a_damaged_job_state_is_an_error() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(one_world::JOBS_DIR).join("j");
        std::fs::create_dir_all(&dir).unwrap();
        let good = job_state(&units);
        let mut huge_slot = good.clone();
        huge_slot["slots"][done_key(&units[1])] = json!(1u64 << 40);
        for (k, v) in [
            ("area_base", json!(u32::MAX)),
            ("area_base", json!(1u64 << 33)),
        ]
        .into_iter()
        .map(|(k, v)| {
            let mut s = good.clone();
            s[k] = v;
            (k, s)
        })
        .chain([("slots", huge_slot)])
        {
            std::fs::write(dir.join("job.json"), v.to_string()).unwrap();
            let err = Job::open(tmp.path(), "j".into(), &units, &[], true)
                .err()
                .unwrap();
            assert!(err.contains("damaged"), "{k}: {err}");
        }
        // The last piece's area id reaches u32::MAX exactly: fine.
        let mut edge = good.clone();
        edge["area_base"] = json!(u32::MAX - units.len() as u32 + 1);
        std::fs::write(dir.join("job.json"), edge.to_string()).unwrap();
        assert!(Job::open(tmp.path(), "j".into(), &units, &[], true).is_ok());
    }

    #[test]
    fn a_piece_takes_its_map_ids_when_it_first_starts() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path().join("W");
        let mut args = <Args as clap::Parser>::parse_from(["arnis", "--bbox", &req.arg()]);
        drop(one_world::prepare(&w, &req, &mut args).unwrap());
        let maps = crate::world_utils::WorldLayout::of(&w).maps_dir(&w);
        let next = || crate::map_item::next_map_id(&maps);
        let start = next();
        // Opening the job costs the world no map ids, however many pieces it has.
        let job = Job::open(&w, "j".into(), &units, &[], true).unwrap();
        assert_eq!(next(), start);
        let lease = |job: &Job, i: usize, first: i32| {
            let lease = UnitLease {
                piece: i as u32,
                area_id: 0,
                extending: true,
                first_map_id: first,
                rect: corners(&units[i].rect),
                job: corners(&world),
                tiled: false,
            };
            std::fs::write(job.lease_path(i), json!(lease).to_string()).unwrap();
        };
        // Each piece that starts takes one range; started again, it keeps it.
        for (i, first) in [(2, start), (0, start + MAP_IDS_PER_PIECE)] {
            assert_eq!(job.first_map_id(&w, i).unwrap(), first);
            lease(&job, i, first);
        }
        assert_eq!(next(), start + 2 * MAP_IDS_PER_PIECE);
        let job = Job::open(&w, "j".into(), &units, &[], true).unwrap();
        assert_eq!(job.first_map_id(&w, 2).unwrap(), start);
        assert_eq!(next(), start + 2 * MAP_IDS_PER_PIECE);
        // A prewarm takes none, and the lease it leaves holds none for the build.
        let warm = Job::open(&w, "j".into(), &units, &[], false).unwrap();
        assert_eq!(warm.first_map_id(&w, 1).unwrap(), 0);
        lease(&warm, 1, 0);
        assert_eq!(
            (warm.kept_map_id(1), next()),
            (None, start + 2 * MAP_IDS_PER_PIECE)
        );
        // No room for another piece: an error, not ids past the end.
        crate::map_item::set_map_counter(&w, i32::MAX - MAP_IDS_PER_PIECE).unwrap();
        assert!(job.first_map_id(&w, 3).unwrap_err().contains("no map ids"));
    }

    #[test]
    fn a_resume_names_the_settings_that_changed() {
        let opts = |argv: &[&str]| {
            generation_options(&argv.iter().map(OsString::from).collect::<Vec<_>>())
        };
        let first = opts(&[
            "--one-world",
            "--output-dir",
            "saves",
            "--scale",
            "1",
            "--threads=8",
            "--winter",
            "--spawn-lat",
            "-44.4",
            "--debug",
        ]);
        assert_eq!(
            first,
            ["--one-world", "--scale=1", "--spawn-lat=-44.4", "--winter"]
        );
        // Same settings in another order and on another machine: nothing to say.
        let same = opts(&[
            "--spawn-lat=-44.4",
            "--winter",
            "--threads",
            "2",
            "--scale=1",
        ]);
        assert!(changed_options(&first, &[&same[..], &["--one-world".into()]].concat()).is_empty());
        let other = opts(&["--one-world", "--scale=2", "--spawn-lat=-44.4", "--caves"]);
        assert_eq!(
            changed_options(&first, &other),
            ["--caves", "--scale", "--winter"]
        );
        // What the world keeps from its start, and the spawn, a resume cannot change.
        let pinned = opts(&[
            "--one-world",
            "--scale=1",
            "--winter",
            "--seed",
            "9",
            "--cave-seed=3",
            "--spawn-lng",
            "26.1",
        ]);
        assert!(changed_options(&first, &pinned).is_empty());
        assert_eq!(settings_changed_notice(&[]), None);
        // The window sends its switches as `--name=value`: the notice names the changed ones.
        let was = opts(&["--winter=false", "--river-bed=off", "--interior=true"]);
        let now = opts(&["--winter=true", "--river-bed=v1", "--interior=true"]);
        assert_eq!(
            settings_changed_notice(&changed_options(&was, &now)).unwrap(),
            "Settings changed since this job started: --river-bed, --winter"
        );
        // Kept with a new job, read back on a resume.
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let w = tmp.path().join("W");
        let mut args = <Args as clap::Parser>::parse_from(["arnis", "--bbox", &req.arg()]);
        drop(one_world::prepare(&w, &req, &mut args).unwrap());
        // A prewarm takes the area ids a build would, but reserves none and keeps no state.
        let next_area = || Manifest::load(&w).unwrap().unwrap().next_area_id;
        let area_base = next_area();
        let warm = Job::open(&w, "j".into(), &units, &first, false).unwrap();
        assert_eq!(warm.area_base, area_base);
        assert!(!w.join(one_world::JOBS_DIR).join("j/job.json").exists());
        assert_eq!(next_area(), area_base);
        Job::open(&w, "j".into(), &units, &first, true).unwrap();
        assert_eq!(next_area(), area_base + units.len() as u32);
        let state: Value = serde_json::from_str(
            &std::fs::read_to_string(w.join(one_world::JOBS_DIR).join("j/job.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(state["argv"], json!(first));
    }

    #[test]
    fn a_kept_job_area_plans_the_same_job_after_the_map_rounds_it() {
        let args = <Args as clap::Parser>::parse_from(["arnis"]);
        for (lat, lon, scale, bbox) in [
            (44.4465, 26.099, 1.0, "44.4375,26.0865,44.4555,26.1115"),
            (-33.9, 151.2, 4.0, "-33.95,151.15,-33.9,151.25"),
            (52.5, 13.4, 0.05, "52.4,13.2,52.6,13.6"),
            // Far north of the origin, where a degree of latitude is the most blocks.
            (0.0, 0.0, 1.5, "84.2,10,84.25,10.3"),
        ] {
            let proj = crate::projection::WebMercatorProjection::new(lat, lon, scale);
            let req = LLBBox::from_str(bbox).unwrap();
            let (_, snapped) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
            let first = work_units::plan(&proj, &snapped, &args, UnitSize::N(2)).unwrap();
            let kept = kept_bbox(&proj, &first.requested);
            // The map reports six decimals, and the run snaps what it is given.
            let text = kept.map(|v| format!("{v:.6}")).join(",");
            let back = LLBBox::from_str(&text).unwrap();
            assert_eq!(back.min().lat(), kept[0]);
            assert_eq!(back.max().lng(), kept[3]);
            let (_, again) = crate::projection::snap_bbox_to_chunks(&proj, &back).unwrap();
            let resumed = work_units::plan(&proj, &again, &args, UnitSize::N(2)).unwrap();
            assert_eq!(resumed.name, first.name, "{bbox}");
        }
    }

    #[test]
    fn map_ids_bound_the_pieces_a_job_reserves() {
        let first = crate::map_item::FIRST_MAP_ID;
        assert_eq!(pieces_with_map_ids(first), MAX_PIECES);
        // Earlier jobs' reservations leave less room.
        assert_eq!(pieces_with_map_ids(i32::MAX - 2 * MAP_IDS_PER_PIECE), 2);
        assert_eq!(pieces_with_map_ids(i32::MAX), 0);
    }

    #[test]
    fn only_the_piece_holding_the_spawn_gets_it() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let r = &units[2].rect;
        assert_eq!(spawn_piece(&units, Some((r.min_x(), r.min_z()))), Some(2));
        let far = (world.max_x() + 1000, world.max_z() + 1000);
        assert_eq!(spawn_piece(&units, Some(far)), None);
        assert_eq!(spawn_piece(&units, None), None);
    }

    #[test]
    fn the_job_percentage_follows_the_pieces_done() {
        // 2 of 16 done, two running at a half and a quarter.
        let mut d = vec![0.0; 16];
        d[0] = 1.0;
        d[1] = 1.0;
        d[2] = 0.5;
        d[3] = 0.25;
        assert!((job_percent(&d) - 2.75 / 16.0 * 100.0).abs() < 1e-9);
        assert_eq!(job_percent(&[1.0; 16]), 100.0);
        assert_eq!(job_percent(&[]), 0.0);
    }

    #[test]
    fn a_piece_region_download_notice_is_forwarded() {
        let rec = |line: &str| serde_json::from_str::<Value>(line).unwrap();
        let notice = rec(
            r#"{"v":1,"type":"phase","name":"region_download_fallback: network","progress":null}"#,
        );
        assert_eq!(
            forwarded_notice(&notice),
            Some("region_download_fallback: network")
        );
        let step = rec(r#"{"v":1,"type":"phase","name":"Generating area...","progress":20.0}"#);
        assert_eq!(forwarded_notice(&step), None);
        assert_eq!(
            forwarded_notice(&rec(r#"{"v":1,"type":"progress","progress":3.0}"#)),
            None
        );
    }

    #[test]
    fn a_region_download_fallback_drops_the_pbf_options() {
        let argv = ["--osm-pbf", "geofabrik", "--osm-pbf-url=u", "--scale", "1"];
        let got = without_options(
            argv.map(OsString::from).to_vec(),
            &["--osm-pbf", "--osm-pbf-url"],
            &[],
        );
        assert_eq!(got, ["--scale", "1"].map(OsString::from));
    }

    #[test]
    fn a_piece_gets_the_jobs_options_but_its_own_bbox_and_lease() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = work_units::plan_units(&proj, &world, 1).unwrap();
        let argv = [
            "--one-world",
            "--bbox",
            "-1,-2,3,4",
            "--unit-regions=1",
            "--threads",
            "8",
            "--spawn-lat",
            "-44.4",
            "--output-dir",
            "out dir",
            "--progress",
            "json",
            "--no-update-check",
            "--only-pieces",
            "2,3",
            "--mapillary-token",
            "MLY|secret",
            "--mapillary-token=MLY|secret",
            "--skip-built",
            "--skip-cells=1,2",
            "--stitch-map",
            "--snap-regions",
            "cover",
            "--margin-regions=1",
            "--scale=1",
        ]
        .map(OsString::from);
        let got = |keep| {
            child_args(
                argv.iter().cloned(),
                &units[3].bbox.arg(),
                Path::new("l.json"),
                keep,
            )
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
        };
        let got_without_spawn = got(false);
        assert_eq!(
            got_without_spawn,
            [
                "--one-world",
                "--output-dir",
                "out dir",
                "--no-update-check",
                "--scale=1",
                "--bbox",
                &units[3].bbox.arg(),
                "--one-world-unit",
                "l.json",
                "--progress",
                "json",
            ]
        );
        assert!(got(true).contains(&"--spawn-lat".to_string()));
        // What the piece parses is the bbox that snaps to its rectangle.
        let parsed = LLBBox::from_str(&got_without_spawn[6]).unwrap();
        let (rect, _) = crate::projection::snap_bbox_to_chunks(&proj, &parsed).unwrap();
        assert_eq!(rect.min_x(), units[3].rect.min_x());
        assert_eq!(rect.max_z(), units[3].rect.max_z());
    }

    #[test]
    fn only_failures_that_may_pass_are_retried() {
        let net = Some(progress_json::NETWORK);
        assert!(is_transient(Some(1), net));
        // A reported error, whatever its words, and any crash are not retried.
        assert!(!is_transient(Some(1), None));
        assert!(!is_transient(Some(1), Some("other")));
        assert!(!is_transient(Some(101), net));
        assert!(!is_transient(None, net));
        assert!(!is_transient(Some(0xC000_0005u32 as i32), net));
    }

    #[test]
    fn a_piece_reads_kind_from_its_error_record() {
        let read = |kind| piece_record_of(&progress_json::error_line("timed out", kind).unwrap());
        let net = read(Some(progress_json::NETWORK)).unwrap();
        assert_eq!(net["type"], "error");
        assert!(is_transient(Some(1), net["kind"].as_str()));
        assert!(!is_transient(Some(1), read(None).unwrap()["kind"].as_str()));
        assert!(piece_record_of("Error: timed out").is_none());
    }

    /// Builds the parity area as a job, kills one piece, resumes, and compares with an
    /// uninterrupted job. Needs network, python with nbtlib, a debug build of arnis, which has
    /// the test hook (`ARNIS_BIN`), and chunkdiff.py (`CHUNKDIFF`):
    /// `cargo test scale::tests::resume -- --ignored`
    #[test]
    #[ignore]
    fn resume_after_kill_matches_full_run() {
        let bin = std::env::var("ARNIS_BIN").expect("set ARNIS_BIN to the arnis executable");
        let chunkdiff = std::env::var("CHUNKDIFF").expect("set CHUNKDIFF to chunkdiff.py");
        let tmp = std::env::temp_dir().join("arnis-resume-test");
        let _ = std::fs::remove_dir_all(&tmp);
        let (full, resumed) = (tmp.join("full"), tmp.join("resumed"));
        let arnis = |out: &Path, bbox: &str, extra: &[&str], env: &[(&str, &Path)]| {
            Command::new(&bin)
                .args(["--one-world", "--world-name", "W", "--output-dir"])
                .arg(out)
                .args(["--bbox", bbox])
                .args(extra)
                .envs(env.iter().copied())
                .output()
                .expect("run arnis")
        };
        let job = ["--one-world-workers", "2", "--unit-regions", "1"];
        let area = "44.4450,26.0950,44.4480,26.1030";
        for w in [&full, &resumed] {
            // The seed fixes the frame, as in the parity runs.
            assert!(arnis(w, "44.4464,26.0988,44.4466,26.0992", &[], &[])
                .status
                .success());
        }
        assert!(arnis(&full, area, &job, &[]).status.success());
        // Piece 2 dies with the panic code once, as it starts; piece 1 starts first.
        let fail = tmp.join("fail");
        std::fs::write(&fail, "2 0").unwrap();
        let first = arnis(&resumed, area, &job, &[("ARNIS_TEST_FAIL_PIECE", &fail)]);
        assert!(
            !first.status.success(),
            "the killed piece must fail the job"
        );
        assert!(resumed.join("W/arnis_one_world/jobs").is_dir());
        // The other three pieces kept building past the failed one.
        let jobs = std::fs::read_dir(resumed.join("W/arnis_one_world/jobs")).unwrap();
        let job_dir = jobs.flatten().next().unwrap().path();
        let done = std::fs::read_dir(job_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("done-"))
            .count();
        assert_eq!(done, 3, "{}", String::from_utf8_lossy(&first.stderr));
        assert!(String::from_utf8_lossy(&first.stderr).contains("--only-pieces 2"));
        assert!(arnis(&resumed, area, &job, &[]).status.success());
        let out = Command::new("python")
            .arg(&chunkdiff)
            .arg(full.join("W"))
            .arg(resumed.join("W"))
            .output()
            .expect("run chunkdiff");
        let text = String::from_utf8_lossy(&out.stdout);
        // Every chunk equal once the volatile LastUpdate and InhabitedTime are dropped.
        let number = |after: &str| -> u64 {
            let rest = text.split(after).nth(1).unwrap_or_else(|| panic!("{text}"));
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().unwrap_or_else(|_| panic!("{text}"))
        };
        assert!(text.contains("only_A=0 only_B=0"), "{text}");
        assert_eq!(number("chunks A="), number("'LastUpdate']: "), "{text}");
    }
}
