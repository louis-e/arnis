//! One World: one persistent Java world that every generation extends.
//!
//! The world pins a Web Mercator frame (origin and scale in the manifest next
//! to `level.dat`). Every requested area is snapped outward to whole chunks in
//! that frame and written into the existing region files, so areas generated
//! at different times line up block for block.

use crate::args::Args;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::ElevationAffine;
use crate::projection::{snap_bbox_to_chunks, WebMercatorProjection};
use crate::world_utils::{world_is_locked, SessionLock};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "arnis_one_world.json";
pub const PREVIEW_DIR: &str = "arnis_one_world/previews";
/// Where a job built in pieces keeps its state, one folder per job.
pub const JOBS_DIR: &str = "arnis_one_world/jobs";
pub const DEFAULT_WORLD_NAME: &str = "Arnis One World";
/// 2: merges write into empty region files. Version 1 worlds still hold
/// region template chunks outside their areas and are repaired once.
/// 3: new worlds have the extended build height and a whole-Earth elevation
/// mapping, which older builds would not apply.
pub const MANIFEST_VERSION: u32 = 3;

/// Geometry kept past the area edge, so an element straddling a seam is
/// built whole on both sides.
pub const CLIP_PAD_BLOCKS: i32 = 128;

/// How far past its own chunks a piece of a job builds, writing only its own: what is
/// rooted in a neighbour's ground (a tree, a building's far wall) reaches across the seam
/// as it does in a single run.
// ponytail: fixed reach; something wider than this still loses its far side at a seam.
pub const PIECE_HALO_BLOCKS: i32 = 32;
// Whole chunks, so the piece's own chunk snap gives its build rect back exactly.
const _: () = assert!(PIECE_HALO_BLOCKS % 16 == 0);

/// What a piece over `rect` builds: `rect` grown by `PIECE_HALO_BLOCKS`, but never past
/// the job's `selection`, which a single run would not build either.
pub fn piece_build_rect(rect: &XZBBox, selection: &XZBBox) -> XZBBox {
    let h = PIECE_HALO_BLOCKS;
    XZBBox::rect_from_min_max(
        (rect.min_x() - h).max(selection.min_x()),
        (rect.min_z() - h).max(selection.min_z()),
        (rect.max_x() + h).min(selection.max_x()),
        (rect.max_z() + h).min(selection.max_z()),
    )
    .unwrap_or_else(|_| rect.clone())
}

const MAX_ABS_LAT: f64 = 85.0;

/// Rounds to `decimals` places. serde_json reads such short decimals back
/// exactly, but not every f64 it writes, and the first run has to use the same
/// numbers as the runs that load them.
fn stable(v: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (v * f).round() / f
}

/// Ground data is fetched this far past the area and cropped again, so the
/// smoothing passes (widest: built-up Gaussian, ~90 m) agree across seams, out
/// to the end of the geometry kept past the edge.
pub fn ground_pad_blocks(scale: f64) -> i32 {
    ((100.0 * scale).ceil() as i32).max(96) + CLIP_PAD_BLOCKS
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GeneratedArea {
    pub id: u32,
    /// Unix seconds.
    pub generated_at: u64,
    pub arnis_version: String,
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
    pub preview: Option<String>,
    /// Set when this area is one piece of a job built in pieces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece: Option<PieceRef>,
}

/// Which piece of which job built an area.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PieceRef {
    /// The job, named by its block rectangle and piece size.
    pub job: String,
    /// Position in the job's plan (`WorkUnit::index`).
    pub index: u32,
}

impl GeneratedArea {
    fn is_inside(&self, other: &XZBBox) -> bool {
        other.min_x() <= self.min_x
            && other.min_z() <= self.min_z
            && other.max_x() >= self.max_x
            && other.max_z() >= self.max_z
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    pub version: u32,
    pub created_with: String,
    /// Unix seconds.
    pub created_at: u64,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
    pub ground_level: i32,
    pub terrain: bool,
    pub disable_height_limit: bool,
    pub aws_only_elevation: bool,
    /// Already folded into `elevation`; kept so later areas are told which one applies.
    #[serde(default = "real_height", skip_serializing_if = "is_real_height")]
    pub height_multiplier: f64,
    /// `--cave-seed` and `--cave-datum-y` of the first area, so every area's caves, ores and
    /// deepslate line up at the borders. Absent when the first area gave none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cave_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cave_datum_y: Option<i32>,
    /// Metre to Y mapping shared by every area. Set at creation; worlds from
    /// before version 3 take it from their first terrain area.
    pub elevation: Option<ElevationAffine>,
    /// `--seed` of the first area; later areas and pieces build with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub next_area_id: u32,
    pub areas: Vec<GeneratedArea>,
    /// The place the world shows, set by the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place_name: Option<String>,
    /// Jobs built in pieces, kept after their folder goes so the window can rebuild them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<JobRecord>,
}

/// A job built in pieces: the window's settings for it and how far it got.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct JobRecord {
    pub name: String,
    /// The flags the window sent, without paths.
    pub settings: Vec<String>,
    /// The area the job was started on, `[south, west, north, east]` in degrees.
    pub bbox: Option<[f64; 4]>,
    pub status: JobStatus,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct JobStatus {
    pub done: u32,
    pub total: u32,
    pub finished: bool,
    /// Unix milliseconds.
    pub last_run_ms: u64,
}

/// Records how far job `name` got. `settings` and `bbox` replace the kept ones when given.
pub fn record_job(
    world_dir: &Path,
    name: &str,
    settings: Option<&[String]>,
    bbox: Option<[f64; 4]>,
    done: u32,
    total: u32,
    finished: bool,
) -> Result<(), String> {
    let mut manifest = Manifest::load(world_dir)?.ok_or("The One World vanished.")?;
    let i = match manifest.jobs.iter().position(|j| j.name == name) {
        Some(i) => i,
        None => {
            manifest.jobs.push(JobRecord {
                name: name.to_string(),
                ..Default::default()
            });
            manifest.jobs.len() - 1
        }
    };
    let job = &mut manifest.jobs[i];
    if let Some(s) = settings {
        job.settings = s.to_vec();
    }
    if bbox.is_some() {
        job.bbox = bbox;
    }
    job.status = JobStatus {
        done,
        total,
        finished,
        last_run_ms: unix_now_ms(),
    };
    manifest.save(world_dir)
}

/// The world's jobs: the recorded ones, then job folders from before jobs were recorded,
/// counted from their own files.
pub fn job_records(world_dir: &Path, manifest: &Manifest) -> Vec<JobRecord> {
    let mut jobs = manifest.jobs.clone();
    let Ok(dirs) = std::fs::read_dir(world_dir.join(JOBS_DIR)) else {
        return jobs;
    };
    for dir in dirs.flatten().filter(|d| d.path().is_dir()) {
        let name = dir.file_name().to_string_lossy().into_owned();
        if jobs.iter().any(|j| j.name == name) {
            continue;
        }
        let state = dir.path().join("job.json");
        let total = std::fs::read_to_string(&state)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["slots"].as_object().map(|s| s.len() as u32))
            .unwrap_or(0);
        let done = std::fs::read_dir(dir.path()).map_or(0, |d| {
            d.flatten()
                .filter(|f| f.file_name().to_string_lossy().starts_with("done-"))
                .count() as u32
        });
        let last_run_ms = std::fs::metadata(&state)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        jobs.push(JobRecord {
            name,
            settings: Vec::new(),
            bbox: None,
            status: JobStatus {
                done,
                total,
                finished: false,
                last_run_ms,
            },
        });
    }
    jobs
}

fn real_height() -> f64 {
    1.0
}

fn is_real_height(multiplier: &f64) -> bool {
    *multiplier == 1.0
}

impl Manifest {
    fn new(args: &Args, origin_lat: f64, origin_lon: f64) -> Self {
        let scale = stable(args.scale, 9);
        let height_multiplier = stable(args.height_multiplier, 6);
        Self {
            version: MANIFEST_VERSION,
            created_with: format!("arnis {}", env!("CARGO_PKG_VERSION")),
            created_at: unix_now(),
            origin_lat: stable(origin_lat, 7),
            origin_lon: stable(origin_lon, 7),
            scale,
            ground_level: args.ground_level,
            terrain: args.terrain(),
            disable_height_limit: args.disable_height_limit,
            aws_only_elevation: args.aws_only_elevation,
            height_multiplier,
            cave_seed: args.cave_seed,
            cave_datum_y: args.cave_datum_y,
            // One section up, so water carved at the lowest level stays above the floor.
            elevation: args.terrain().then(|| {
                let mut e = ElevationAffine::whole_earth(
                    scale * height_multiplier,
                    crate::ground::min_ground_level_for(args) + 16,
                    crate::ground::extended_max_y_for(args),
                );
                if let Some(top) = e.soft_top.as_mut() {
                    top.knee_m = stable(top.knee_m, 6);
                    // Down, so Everest stays under the ceiling.
                    top.width_blocks = stable(top.width_blocks - 5e-7, 6);
                }
                e
            }),
            seed: args.seed,
            next_area_id: 1,
            areas: Vec::new(),
            place_name: None,
            jobs: Vec::new(),
        }
    }

    pub fn projection(&self) -> WebMercatorProjection {
        WebMercatorProjection::new(self.origin_lat, self.origin_lon, self.scale)
    }

    pub fn path_in(world_dir: &Path) -> PathBuf {
        world_dir.join(MANIFEST_FILE)
    }

    pub fn load(world_dir: &Path) -> Result<Option<Self>, String> {
        let path = Self::path_in(world_dir);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a valid One World manifest: {e}", path.display()))?;
        if manifest.version > MANIFEST_VERSION {
            return Err(format!(
                "{} was written by a newer Arnis (manifest version {}); update Arnis to extend this world.",
                path.display(),
                manifest.version
            ));
        }
        manifest
            .validate()
            .map_err(|e| format!("{} is damaged: {e}", path.display()))?;
        Ok(Some(manifest))
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.origin_lat.is_finite() && self.origin_lat.abs() <= MAX_ABS_LAT) {
            return Err(format!("origin latitude {}", self.origin_lat));
        }
        if !(self.origin_lon.is_finite() && self.origin_lon.abs() <= 180.0) {
            return Err(format!("origin longitude {}", self.origin_lon));
        }
        crate::args::validate_scale(self.scale)?;
        crate::args::validate_height_multiplier(self.height_multiplier)?;
        if let Some(e) = &self.elevation {
            let soft_top_ok = e.soft_top.is_none_or(|t| {
                t.knee_m.is_finite() && t.width_blocks.is_finite() && t.width_blocks > 0.0
            });
            if !(e.min_height_m.is_finite()
                && e.blocks_per_meter.is_finite()
                && e.blocks_per_meter >= 0.0
                && soft_top_ok)
            {
                return Err("elevation mapping".to_string());
            }
        }
        if let Some(a) = self
            .areas
            .iter()
            .find(|a| a.min_x > a.max_x || a.min_z > a.max_z)
        {
            return Err(format!("area {} has an empty rectangle", a.id));
        }
        Ok(())
    }

    pub fn save(&self, world_dir: &Path) -> Result<(), String> {
        let path = Self::path_in(world_dir);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize the One World manifest: {e}"))?;
        crate::world_utils::replace_file_atomically(&path, text.as_bytes())
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    /// The areas' union as `[south, west, north, east]` in degrees; none without areas.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let mut it = self.areas.iter();
        let a = it.next()?;
        Some(
            it.fold([a.min_lat, a.min_lon, a.max_lat, a.max_lon], |b, a| {
                [
                    b[0].min(a.min_lat),
                    b[1].min(a.min_lon),
                    b[2].max(a.max_lat),
                    b[3].max(a.max_lon),
                ]
            }),
        )
    }

    pub fn extent(&self) -> Option<XZBBox> {
        let first = self.areas.first()?;
        let mut min_x = first.min_x;
        let mut min_z = first.min_z;
        let mut max_x = first.max_x;
        let mut max_z = first.max_z;
        for a in &self.areas[1..] {
            min_x = min_x.min(a.min_x);
            min_z = min_z.min(a.min_z);
            max_x = max_x.max(a.max_x);
            max_z = max_z.max(a.max_z);
        }
        XZBBox::rect_from_min_max(min_x, min_z, max_x, max_z).ok()
    }
}

/// Chunks inside `rect` that already exist in the world's region files,
/// read from the region headers alone.
pub fn existing_chunks(world_dir: &Path, rect: &XZBBox) -> u64 {
    let (cx0, cz0) = (rect.min_x() >> 4, rect.min_z() >> 4);
    let (cx1, cz1) = (rect.max_x() >> 4, rect.max_z() >> 4);
    let region_dir = crate::world_utils::WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("region");
    let mut count = 0;
    for rz in (cz0 >> 5)..=(cz1 >> 5) {
        for rx in (cx0 >> 5)..=(cx1 >> 5) {
            let path = region_dir.join(format!("r.{rx}.{rz}.mca"));
            let mut header = [0u8; 4096];
            let read = std::fs::File::open(&path).and_then(|mut f| f.read_exact(&mut header));
            if read.is_err() {
                continue;
            }
            for lz in 0..32 {
                for lx in 0..32 {
                    let (cx, cz) = (rx * 32 + lx, rz * 32 + lz);
                    let i = 4 * (lx + lz * 32) as usize;
                    if (cx0..=cx1).contains(&cx)
                        && (cz0..=cz1).contains(&cz)
                        && header[i..i + 4] != [0; 4]
                    {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// Block (0, 0) of a world created for `requested`: `--origin`, or the request's centre.
fn new_origin(requested: &LLBBox, args: &Args) -> (f64, f64) {
    args.origin.unwrap_or((
        (requested.min().lat() + requested.max().lat()) / 2.0,
        (requested.min().lng() + requested.max().lng()) / 2.0,
    ))
}

/// The frame `requested` lands in: the world's own, or for a world that does
/// not exist yet the one `prepare` would create. Reads only; takes no lock.
pub fn frame_for(
    world_dir: &Path,
    requested: &LLBBox,
    args: &Args,
) -> Result<WebMercatorProjection, String> {
    Ok(match Manifest::load(world_dir)? {
        Some(manifest) => manifest.projection(),
        None => {
            let (lat, lon) = new_origin(requested, args);
            Manifest::new(args, lat, lon).projection()
        }
    })
}

/// The One World a run lands in, carried in `Args::one_world_run`.
#[derive(Clone, Debug)]
pub struct RunContext {
    pub world_dir: PathBuf,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub extending: bool,
    pub elevation: Option<ElevationAffine>,
    /// Chunks of this area that already exist and are replaced.
    pub replaced_chunks: u64,
    pub area_id: u32,
    /// Set when this run builds one piece of a job.
    pub piece: Option<PieceRef>,
    /// Set in a piece of a job, whose coordinator holds the world's lock and records the area.
    pub unit: Option<UnitLease>,
}

/// What a job's coordinator tells one of its pieces, in the file `--one-world-unit` names.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UnitLease {
    /// Position in the job's plan.
    pub piece: u32,
    pub area_id: u32,
    /// False for the one piece that makes the world's map item and settings.
    pub extending: bool,
    /// The piece's own range of map ids, so pieces running together never share one.
    pub first_map_id: i32,
    /// The piece's own block rectangle: it builds past it and writes only inside.
    pub rect: [i32; 4],
    /// The job's whole block rectangle, which metadata.json describes.
    pub job: [i32; 4],
    /// Whether a single run of the whole job would build in parallel tiles.
    pub tiled: bool,
}

impl RunContext {
    pub fn preview_path(&self) -> PathBuf {
        self.world_dir
            .join(PREVIEW_DIR)
            .join(format!("area-{}.png", self.area_id))
    }
}

/// A resolved run. Holds the world's session lock until dropped.
pub struct Session {
    pub created: bool,
    pub llbbox: LLBBox,
    pub lock: SessionLock,
}

fn compatibility_errors(manifest: &Manifest, args: &Args) -> Vec<String> {
    let mut errors = Vec::new();
    if (manifest.scale - args.scale).abs() > 1e-9 {
        errors.push(format!(
            "world scale {:.2} does not match the world's {:.2}",
            args.scale, manifest.scale
        ));
    }
    if manifest.ground_level != args.ground_level {
        errors.push(format!(
            "ground level {} does not match the world's {}",
            args.ground_level, manifest.ground_level
        ));
    }
    if manifest.terrain != args.terrain() {
        errors.push(format!(
            "the world was generated {} terrain, so this area must be too",
            if manifest.terrain { "with" } else { "without" }
        ));
    }
    if args
        .cave_seed
        .is_some_and(|s| manifest.cave_seed != Some(s))
    {
        errors.push(match manifest.cave_seed {
            Some(s) => format!("--cave-seed does not match the world's {s}"),
            None => "--cave-seed was not used when the world was created".to_string(),
        });
    }
    if args
        .cave_datum_y
        .is_some_and(|y| manifest.cave_datum_y != Some(y))
    {
        errors.push(match manifest.cave_datum_y {
            Some(y) => format!("--cave-datum-y does not match the world's {y}"),
            None => "--cave-datum-y was not used when the world was created".to_string(),
        });
    }
    errors
}

fn open_in_minecraft(world_dir: &Path) -> String {
    format!(
        "The One World at {} is open in Minecraft. Leave the world (or close the game) and try again.",
        world_dir.display()
    )
}

/// A world's looks are a pure function of the seed, so it keeps its first area's: a later
/// area or piece with another would not match its neighbours at the seam.
fn keep_seed(manifest: &Manifest, args: &mut Args) {
    if args.seed.is_some() && args.seed != manifest.seed {
        let kept = manifest.seed.map_or("none".into(), |s| s.to_string());
        println!("Note: One World keeps the seed it was created with ({kept}).");
    }
    args.seed = manifest.seed;
}

/// Opens or creates the One World at `world_dir`, locks it, and points `args`
/// at the world's frame.
pub fn prepare(world_dir: &Path, requested: &LLBBox, args: &mut Args) -> Result<Session, String> {
    if args.bedrock || args.luanti {
        return Err("One World is available for Java Edition worlds only.".to_string());
    }
    if !args.body.is_earth() {
        return Err("One World is available for Earth only.".to_string());
    }
    if args.rotation.abs() > f64::EPSILON {
        return Err(
            "One World keeps the world aligned to real-world coordinates, so rotation must be 0."
                .to_string(),
        );
    }
    if requested.min().lat() < -MAX_ABS_LAT || requested.max().lat() > MAX_ABS_LAT {
        return Err(format!(
            "One World covers latitudes up to {MAX_ABS_LAT} degrees north and south."
        ));
    }

    let fresh_dir = !world_dir.exists();
    if !fresh_dir && Manifest::load(world_dir)?.is_none() {
        let empty = std::fs::read_dir(world_dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(format!(
                "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
                world_dir.display(),
                MANIFEST_FILE
            ));
        }
    }
    if world_is_locked(world_dir) {
        // A live server a crashed Arnis left running holds the lock too.
        crate::live::stop_leftover_server(world_dir);
    }
    if world_is_locked(world_dir) {
        return Err(open_in_minecraft(world_dir));
    }
    std::fs::create_dir_all(world_dir)
        .map_err(|e| format!("Failed to create {}: {e}", world_dir.display()))?;
    let lock = SessionLock::acquire(world_dir).map_err(|_| open_in_minecraft(world_dir))?;
    // Live mode can leave stubs behind if it ended badly; a run must not merge into those.
    if !fresh_dir {
        crate::live::recover(world_dir)?;
    }
    let mut owned = false;
    match resolve(world_dir, requested, args, &mut owned) {
        Ok((created, llbbox)) => Ok(Session {
            created,
            llbbox,
            lock,
        }),
        Err(e) => {
            drop(lock);
            if fresh_dir {
                // Everything in it is ours once the check under the lock passed.
                // Before that only an empty folder is removed.
                let _ = if owned {
                    std::fs::remove_dir_all(world_dir)
                } else {
                    std::fs::remove_dir(world_dir)
                };
            }
            Err(e)
        }
    }
}

/// Opens the world for one piece of a job. The coordinator holds the world's lock and
/// has created the world, so this takes no lock and creates nothing; the lease it
/// reads decides the area id and whether the piece makes the world's extras.
pub fn prepare_unit(
    world_dir: &Path,
    requested: &LLBBox,
    args: &mut Args,
    lease: &Path,
) -> Result<LLBBox, String> {
    let lease: UnitLease = std::fs::read_to_string(lease)
        .map_err(|e| format!("Failed to read {}: {e}", lease.display()))
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))?;
    if Manifest::load(world_dir)?.is_none() {
        return Err(format!("{} is not a One World.", world_dir.display()));
    }
    crate::scale::child::fail_for_test(lease.piece as usize);
    crate::map_item::leave_counter_to_coordinator();
    let (_, llbbox) = resolve(world_dir, requested, args, &mut false)?;
    if let Some(run) = args.one_world_run.as_mut() {
        run.area_id = lease.area_id;
        run.extending = lease.extending;
        run.unit = Some(lease);
    }
    Ok(llbbox)
}

/// The part of `prepare` that runs under the world's lock.
/// `owned` is set once the folder is known to hold nothing but this run's files.
fn resolve(
    world_dir: &Path,
    requested: &LLBBox,
    args: &mut Args,
    owned: &mut bool,
) -> Result<(bool, LLBBox), String> {
    let (mut manifest, created) = match Manifest::load(world_dir)? {
        Some(manifest) => {
            if !world_dir.join("level.dat").is_file() {
                return Err(format!(
                    "{} has a One World manifest but no level.dat; the world seems damaged.",
                    world_dir.display()
                ));
            }
            let errors = compatibility_errors(&manifest, args);
            if !errors.is_empty() {
                return Err(format!(
                    "This area cannot be added to the One World at {}: {}. Change the setting, or use another world name to start a new One World.",
                    world_dir.display(),
                    errors.join("; ")
                ));
            }
            if manifest.disable_height_limit != args.disable_height_limit {
                println!(
                    "Note: One World keeps the build height it was created with ({}).",
                    if manifest.disable_height_limit {
                        "extended"
                    } else {
                        "vanilla"
                    }
                );
                args.disable_height_limit = manifest.disable_height_limit;
            }
            if manifest.height_multiplier != args.height_multiplier {
                println!(
                    "Note: One World keeps the terrain height multiplier it was created with ({}x).",
                    manifest.height_multiplier
                );
                args.height_multiplier = manifest.height_multiplier;
            }
            if let Some((lat, lon)) = args.origin {
                if (stable(lat, 7), stable(lon, 7)) != (manifest.origin_lat, manifest.origin_lon) {
                    println!(
                        "Note: --origin {lat},{lon} is ignored; the One World keeps the origin it was created with ({},{}).",
                        manifest.origin_lat, manifest.origin_lon
                    );
                }
            }
            if args.caves {
                if let (None, Some(seed)) = (args.cave_seed, manifest.cave_seed) {
                    println!("Note: One World keeps the cave seed it was created with ({seed}).");
                }
                if let (None, Some(y)) = (args.cave_datum_y, manifest.cave_datum_y) {
                    println!("Note: One World keeps the cave floor it was created with (Y {y}).");
                }
            }
            args.cave_seed = manifest.cave_seed;
            args.cave_datum_y = manifest.cave_datum_y;
            (manifest, false)
        }
        None => {
            // Checked again under the lock: the folder may have filled up since.
            let foreign = std::fs::read_dir(world_dir)
                .map_err(|e| format!("Failed to read {}: {e}", world_dir.display()))?
                .filter_map(Result::ok)
                .any(|entry| entry.file_name() != "session.lock");
            if foreign {
                return Err(format!(
                    "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
                    world_dir.display(),
                    MANIFEST_FILE
                ));
            }
            *owned = true;
            if args.vanilla_height {
                return Err(
                    "A new One World always has the extended build height, so any place on Earth fits. Leave out --vanilla-height."
                        .to_string(),
                );
            }
            // Room for any place on Earth, so the first area does not limit later ones.
            if !args.disable_height_limit {
                println!(
                    "Note: One World uses the extended build height, so any place on Earth fits."
                );
                args.disable_height_limit = true;
            }
            let (origin_lat, origin_lon) = new_origin(requested, args);
            (Manifest::new(args, origin_lat, origin_lon), true)
        }
    };

    args.scale = manifest.scale;
    keep_seed(&manifest, args);
    let (xzbbox, llbbox) = snap_bbox_to_chunks(&manifest.projection(), requested)?;

    if created {
        let name = world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(DEFAULT_WORLD_NAME)
            .to_string();
        crate::world_utils::write_world_skeleton(world_dir, &name, false)?;
        manifest.save(world_dir)?;
    }

    if manifest.version < 2 {
        let removed = crate::world_editor::java::drop_misplaced_chunks(world_dir).map_err(|e| {
            format!(
                "Failed to repair the One World at {}: {e}",
                world_dir.display()
            )
        })?;
        println!(
            "One World: repaired a world from an earlier build ({removed} stray chunks dropped)."
        );
        manifest.version = 2;
        manifest.save(world_dir)?;
    }

    if manifest.aws_only_elevation != args.aws_only_elevation {
        println!(
            "Note: One World keeps the elevation source it was created with ({}).",
            if manifest.aws_only_elevation {
                "legacy AWS terrain"
            } else {
                "high-resolution terrain"
            }
        );
        args.aws_only_elevation = manifest.aws_only_elevation;
    }

    let replaced = if created {
        0
    } else {
        existing_chunks(world_dir, &xzbbox)
    };
    let extending = !manifest.areas.is_empty();
    let area_id = manifest.next_area_id;

    args.one_world_run = Some(RunContext {
        world_dir: world_dir.to_path_buf(),
        origin_lat: manifest.origin_lat,
        origin_lon: manifest.origin_lon,
        extending,
        elevation: manifest.elevation,
        replaced_chunks: replaced,
        area_id,
        piece: None,
        unit: None,
    });
    args.projection = crate::projection::ProjectionKind::WebMercator;
    args.bbox = Some(llbbox);
    // Voxy and DH rebuild their LOD databases from the regions of one run, and
    // both facade sources replace the world's resource pack on every run.
    if args.voxy_lod {
        println!("Note: the Voxy LOD cache is off in One World mode.");
        args.voxy_lod = false;
    }
    if args.dh_lod {
        println!("Note: the Distant Horizons LOD cache is off in One World mode.");
        args.dh_lod = false;
    }
    if args.mapillary_facade_mode.places_displays() && args.mapillary_facades_wanted() {
        println!("Note: One World builds Mapillary facades as blocks; photo panels are off.");
        args.mapillary_facade_mode = crate::args::FacadeMode::Blocks;
    }
    if args.building_facades {
        println!("Note: the preset building facades are off in One World mode.");
        args.building_facades = false;
    }
    // A flat plane sits at the first area's lowest ground, which later areas need not share.
    if args.world_type == crate::args::WorldType::Flat {
        println!("Note: One World is always a void world; the flat world type is off.");
        args.world_type = crate::args::WorldType::Void;
    }
    // level.dat and the map item are written by the first area only.
    if extending {
        println!(
            "Note: One World keeps the game mode, time of day, world type, mob spawning and map item of its first area."
        );
    }
    args.map_preview = true;

    println!(
        "One World: {} {} at {}",
        if created {
            "created"
        } else if extending {
            "extending"
        } else {
            "using"
        },
        world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(DEFAULT_WORLD_NAME),
        world_dir.display()
    );
    println!(
        "  area #{area_id}: blocks x {}..={} z {}..={} ({} x {} chunks){}",
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
        (xzbbox.max_x() - xzbbox.min_x() + 1) / 16,
        (xzbbox.max_z() - xzbbox.min_z() + 1) / 16,
        if replaced > 0 {
            format!(", {replaced} existing chunks will be replaced")
        } else {
            String::new()
        }
    );
    // Ground scale changes with latitude; far from the origin a request
    // becomes a much larger world than its size on the map suggests.
    let centre_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
    let density = manifest.origin_lat.to_radians().cos() / centre_lat.to_radians().cos();
    if !(0.8..=1.25).contains(&density) {
        println!(
            "Note: this far from the world's origin one metre is {:.2} blocks instead of {:.2}.",
            args.scale * density,
            args.scale
        );
    }

    Ok((created, llbbox))
}

/// Records a finished area and drops the ones it fully covers.
pub fn record_area(
    run: &RunContext,
    llbbox: &LLBBox,
    xzbbox: &XZBBox,
    elevation: Option<ElevationAffine>,
    preview: Option<&Path>,
) -> Result<(), String> {
    let mut manifest = Manifest::load(&run.world_dir)?.ok_or_else(|| {
        format!(
            "{} vanished during generation",
            Manifest::path_in(&run.world_dir).display()
        )
    })?;
    if manifest.elevation.is_none() {
        manifest.elevation = elevation;
    }
    let preview_rel = preview.and_then(|p| {
        p.strip_prefix(&run.world_dir)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    });
    let (covered, kept): (Vec<GeneratedArea>, Vec<GeneratedArea>) =
        std::mem::take(&mut manifest.areas)
            .into_iter()
            .partition(|a| a.is_inside(xzbbox));
    manifest.areas = kept;
    manifest.areas.push(GeneratedArea {
        id: run.area_id,
        generated_at: unix_now(),
        arnis_version: env!("CARGO_PKG_VERSION").to_string(),
        min_x: xzbbox.min_x(),
        min_z: xzbbox.min_z(),
        max_x: xzbbox.max_x(),
        max_z: xzbbox.max_z(),
        min_lat: llbbox.min().lat(),
        min_lon: llbbox.min().lng(),
        max_lat: llbbox.max().lat(),
        max_lon: llbbox.max().lng(),
        preview: preview_rel.clone(),
        piece: run.piece.clone(),
    });
    manifest.next_area_id = manifest.next_area_id.max(run.area_id.saturating_add(1));
    manifest.save(&run.world_dir)?;
    for old in covered {
        if let Some(p) = old.preview.filter(|p| Some(p) != preview_rel.as_ref()) {
            if let Some(path) = safe_preview_path(&run.world_dir, &p) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

/// `--world-border`: the border around `rect`, the area just built, or for a
/// One World around every area it holds. A failure is a warning: the world
/// itself is written.
pub fn apply_world_border(world_dir: &Path, rect: &XZBBox) {
    let all = Manifest::load(world_dir)
        .ok()
        .flatten()
        .and_then(|m| m.extent());
    let rect = all.as_ref().unwrap_or(rect);
    match crate::world_utils::set_world_border(world_dir, rect) {
        Ok(()) => println!(
            "World border set around {},{} to {},{}.",
            rect.min_x(),
            rect.min_z(),
            rect.max_x(),
            rect.max_z()
        ),
        Err(e) => eprintln!("Warning: Failed to set the world border: {e}"),
    }
}

/// A new One World centred on `(origin_lat, origin_lon)` with no areas yet, for live mode,
/// whose tiles then line up with the world's region grid from the first one on.
pub fn create_empty(
    world_dir: &Path,
    args: &mut Args,
    origin_lat: f64,
    origin_lon: f64,
) -> Result<(), String> {
    if !(origin_lat.is_finite() && origin_lat.abs() <= MAX_ABS_LAT) {
        return Err(format!(
            "One World covers latitudes up to {MAX_ABS_LAT} degrees north and south."
        ));
    }
    // Live mode holds the lock on the folder it creates the world in.
    let occupied = std::fs::read_dir(world_dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .any(|e| e.file_name() != "session.lock")
        })
        .unwrap_or(false);
    if occupied {
        return Err(format!(
            "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
            world_dir.display(),
            MANIFEST_FILE
        ));
    }
    args.disable_height_limit = true;
    args.vanilla_height = false;
    let name = world_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(DEFAULT_WORLD_NAME)
        .to_string();
    let made = (|| {
        crate::world_utils::write_world_skeleton(world_dir, &name, false)?;
        Manifest::new(args, origin_lat, origin_lon).save(world_dir)?;
        let (min_y, max_y) = crate::world_utils::TALL_DATAPACK_FULL_RANGE;
        crate::world_utils::install_tall_datapack(world_dir, min_y, max_y)?;
        crate::world_utils::apply_java_world_settings(
            world_dir,
            args.gamemode,
            args.world_time,
            crate::args::WorldType::Void,
            args.mob_spawning,
        )
    })();
    if made.is_err() {
        let _ = std::fs::remove_dir_all(world_dir);
    }
    made
}

/// Records a live tile as an area, taking its preview along.
pub fn record_live_tile(
    world_dir: &Path,
    llbbox: &LLBBox,
    xzbbox: &XZBBox,
    preview: Option<&Path>,
) -> Result<(), String> {
    let manifest = Manifest::load(world_dir)?.ok_or_else(|| {
        format!(
            "{} vanished during live mode",
            Manifest::path_in(world_dir).display()
        )
    })?;
    let area_id = manifest.next_area_id;
    let preview = preview.and_then(|src| {
        let dir = world_dir.join(PREVIEW_DIR);
        std::fs::create_dir_all(&dir).ok()?;
        let dst = dir.join(format!("area-{area_id}.png"));
        std::fs::rename(src, &dst)
            .or_else(|_| std::fs::copy(src, &dst).map(|_| ()))
            .ok()?;
        Some(dst)
    });
    let run = RunContext {
        world_dir: world_dir.to_path_buf(),
        origin_lat: manifest.origin_lat,
        origin_lon: manifest.origin_lon,
        extending: true,
        elevation: manifest.elevation,
        replaced_chunks: 0,
        area_id,
        piece: None,
        unit: None,
    };
    record_area(&run, llbbox, xzbbox, manifest.elevation, preview.as_deref())
}

/// Stores the elevation mapping as soon as the first terrain area has one,
/// so a run that fails later cannot leave chunks behind on another mapping.
pub fn remember_elevation(run: &RunContext, elevation: ElevationAffine) -> Result<(), String> {
    if run.elevation.is_some() {
        return Ok(());
    }
    let Some(mut manifest) = Manifest::load(&run.world_dir)? else {
        return Ok(());
    };
    if manifest.elevation.is_none() {
        manifest.elevation = Some(elevation);
        manifest.save(&run.world_dir)?;
    }
    Ok(())
}

/// Manifests can come with downloaded worlds, so their preview paths are
/// only followed inside the preview folder.
pub fn safe_preview_path(world_dir: &Path, rel: &str) -> Option<PathBuf> {
    let name = rel.strip_prefix(PREVIEW_DIR)?.strip_prefix('/')?;
    let valid = !name.is_empty()
        && name.ends_with(".png")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.contains("..");
    valid.then(|| world_dir.join(PREVIEW_DIR).join(name))
}

/// What [`remove_cells`] did.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct RemoveReport {
    /// The job whose cells these are.
    pub job: String,
    /// Cells whose areas left the manifest, by plan index.
    pub removed: Vec<u32>,
    /// Cells asked for that have no area in the world.
    pub not_built: Vec<u32>,
    /// Files deleted, relative to the world folder, `/`-separated.
    pub deleted: Vec<String>,
    /// Region files of the removed cells kept because an area outside them lies there too.
    pub kept: Vec<String>,
}

/// The region files `(rx, rz)` an area lies in.
fn area_regions(a: &GeneratedArea) -> impl Iterator<Item = (i32, i32)> {
    let r = |v: i32| v.div_euclid(512);
    let (x0, x1, z0, z1) = (r(a.min_x), r(a.max_x), r(a.min_z), r(a.max_z));
    (z0..=z1).flat_map(move |z| (x0..=x1).map(move |x| (x, z)))
}

/// Takes `cells` (plan indexes) of `job` (default: the job run last) out of the world: their
/// region files, with the `entities/` and `poi/` files beside them, and their previews are
/// deleted, their areas leave the manifest and the job counts them as not built. A region
/// file another area also lies in is kept. Refused while a job or Minecraft has the world.
pub fn remove_cells(
    world_dir: &Path,
    job: Option<&str>,
    cells: &[u32],
) -> Result<RemoveReport, String> {
    if crate::scale::runs_on(world_dir) {
        return Err("A job is building this world; stop it first.".to_string());
    }
    let _lock = SessionLock::acquire(world_dir).map_err(|_| {
        "The world is in use (open in Minecraft, or another run writes it); close it first."
            .to_string()
    })?;
    let mut manifest = Manifest::load(world_dir)?
        .ok_or_else(|| format!("{} is not a One World.", world_dir.display()))?;
    let job = match job {
        Some(j) => j.to_string(),
        None => manifest
            .jobs
            .iter()
            .max_by_key(|j| j.status.last_run_ms)
            .map(|j| j.name.clone())
            .or_else(|| {
                manifest
                    .areas
                    .iter()
                    .rev()
                    .find_map(|a| Some(a.piece.as_ref()?.job.clone()))
            })
            .ok_or("This world has no job built in cells.")?,
    };
    let (gone, kept): (Vec<_>, Vec<_>) =
        std::mem::take(&mut manifest.areas)
            .into_iter()
            .partition(|a| {
                a.piece
                    .as_ref()
                    .is_some_and(|p| p.job == job && cells.contains(&p.index))
            });
    let removed: std::collections::BTreeSet<u32> = gone
        .iter()
        .flat_map(|a| &a.piece)
        .map(|p| p.index)
        .collect();
    manifest.areas = kept;
    if let Some(j) = manifest.jobs.iter_mut().find(|j| j.name == job) {
        j.status.done = j.status.done.saturating_sub(removed.len() as u32);
        j.status.finished = false;
    }
    // First: should a delete fail, the world shows the cell as not built, and a rebuild
    // writes over what is left.
    manifest.save(world_dir)?;

    let mut report = RemoveReport {
        job: job.clone(),
        removed: removed.iter().copied().collect(),
        not_built: cells
            .iter()
            .copied()
            .filter(|c| !removed.contains(c))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let rel = |p: &Path| {
        let r = p.strip_prefix(world_dir).unwrap_or(p);
        r.to_string_lossy().replace('\\', "/")
    };
    let mut delete = |path: PathBuf| -> Result<(), String> {
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
            report.deleted.push(rel(&path));
        }
        Ok(())
    };
    let others: std::collections::HashSet<(i32, i32)> =
        manifest.areas.iter().flat_map(area_regions).collect();
    let targets: std::collections::BTreeSet<(i32, i32)> =
        gone.iter().flat_map(area_regions).collect();
    let overworld = crate::world_utils::WorldLayout::of(world_dir).overworld_dir(world_dir);
    let mut shared = Vec::new();
    for (rx, rz) in targets {
        let name = format!("r.{rx}.{rz}.mca");
        if others.contains(&(rx, rz)) {
            shared.push(rel(&overworld.join("region").join(&name)));
            continue;
        }
        for folder in ["region", "entities", "poi"] {
            delete(overworld.join(folder).join(&name))?;
        }
    }
    let job_dir = world_dir.join(JOBS_DIR).join(&job);
    for a in &gone {
        if let Some(path) = a
            .preview
            .as_deref()
            .and_then(|p| safe_preview_path(world_dir, p))
        {
            delete(path)?;
        }
        // The job's record of the cell, so a resume builds it again.
        let index = a.piece.as_ref().map_or(0, |p| p.index);
        let key = format!(
            "done-r{}_{}_s",
            a.min_x.div_euclid(512),
            a.min_z.div_euclid(512)
        );
        for f in std::fs::read_dir(&job_dir).into_iter().flatten().flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.starts_with(&key) || name == format!("done-{index}.json") {
                delete(f.path())?;
            }
        }
    }
    report.kept = shared;
    Ok(report)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args_for(bbox: &str, extra: &[&str]) -> Args {
        let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", bbox];
        cmd.extend_from_slice(extra);
        Args::parse_from(cmd)
    }

    const MUNICH: &str = "48.130,11.560,48.145,11.590";

    /// A 3 x 3 grid of cells of 2 x 2 regions, the corner cell clipped mid-region, and one
    /// plain area sharing a region with the middle cell: removing the middle and a corner
    /// deletes exactly their files.
    #[test]
    fn removing_cells_deletes_only_their_region_files() {
        let tmp = tempfile::tempdir().unwrap();
        let world = tmp.path();
        let mut m = Manifest::new(&args_for(MUNICH, &[]), 48.13, 11.56);
        let area = |id: u32, r: [i32; 4], piece: Option<u32>| GeneratedArea {
            id,
            generated_at: 0,
            arnis_version: String::new(),
            min_x: r[0],
            min_z: r[1],
            max_x: r[2],
            max_z: r[3],
            min_lat: 0.0,
            min_lon: 0.0,
            max_lat: 0.0,
            max_lon: 0.0,
            preview: Some(format!("{PREVIEW_DIR}/area-{id}.png")),
            piece: piece.map(|index| PieceRef {
                job: "J".into(),
                index,
            }),
        };
        // Cell `index` sits at grid (gx, gz), both in -1..=1.
        let cell = |index: u32| {
            let (gx, gz) = (index as i32 % 3 - 1, index as i32 / 3 - 1);
            let mut r = [gx * 1024, gz * 1024, gx * 1024 + 1023, gz * 1024 + 1023];
            if index == 0 {
                r[0] += 100;
                r[1] += 100;
            }
            r
        };
        for i in 0..9 {
            m.areas.push(area(i + 1, cell(i), Some(i)));
        }
        // A plain run over region (1, 0), which the middle cell (regions 0..=1) also holds.
        m.areas.push(area(20, [600, 10, 700, 90], None));
        m.jobs.push(JobRecord {
            name: "J".into(),
            status: JobStatus {
                done: 9,
                total: 9,
                finished: true,
                last_run_ms: 1,
            },
            ..Default::default()
        });
        m.save(world).unwrap();
        for d in ["region", "entities", PREVIEW_DIR] {
            std::fs::create_dir_all(world.join(d)).unwrap();
        }
        let mut all = Vec::new();
        for rz in -2..=3 {
            for rx in -2..=3 {
                all.push(format!("region/r.{rx}.{rz}.mca"));
            }
        }
        all.extend(["entities/r.0.0.mca".into(), "entities/r.-2.-2.mca".into()]);
        all.extend((1..=9).map(|id| format!("{PREVIEW_DIR}/area-{id}.png")));
        for f in &all {
            std::fs::write(world.join(f), b"x").unwrap();
        }
        let jobs = world.join(JOBS_DIR).join("J");
        std::fs::create_dir_all(&jobs).unwrap();
        for f in [
            "done-r0_0_s2.json",
            "done-r-2_-2_s2.json",
            "done-r2_2_s2.json",
        ] {
            std::fs::write(jobs.join(f), "{}").unwrap();
        }

        // Refused while the world is locked.
        let held = SessionLock::acquire(world).unwrap();
        let err = remove_cells(world, None, &[4, 0]).unwrap_err();
        assert!(err.contains("in use"), "{err}");
        drop(held);

        let report = remove_cells(world, None, &[4, 0, 0, 12]).unwrap();
        assert_eq!(report.job, "J");
        assert_eq!(report.removed, [0, 4]);
        assert_eq!(report.not_built, [12]);
        assert_eq!(report.kept, ["region/r.1.0.mca"]);
        let mut gone: Vec<String> = [
            "region/r.-2.-2.mca",
            "region/r.-1.-2.mca",
            "region/r.-2.-1.mca",
            "region/r.-1.-1.mca",
            "entities/r.-2.-2.mca",
            "region/r.0.0.mca",
            "entities/r.0.0.mca",
            "region/r.0.1.mca",
            "region/r.1.1.mca",
            &format!("{PREVIEW_DIR}/area-1.png"),
            &format!("{PREVIEW_DIR}/area-5.png"),
            &format!("{JOBS_DIR}/J/done-r0_0_s2.json"),
            &format!("{JOBS_DIR}/J/done-r-2_-2_s2.json"),
        ]
        .map(String::from)
        .to_vec();
        let mut deleted = report.deleted.clone();
        gone.sort();
        deleted.sort();
        assert_eq!(deleted, gone);
        for f in &all {
            assert_eq!(world.join(f).exists(), !gone.contains(f), "{f}");
        }
        assert!(jobs.join("done-r2_2_s2.json").exists());
        let m = Manifest::load(world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 8);
        assert!(m
            .areas
            .iter()
            .all(|a| a.piece.as_ref().is_none_or(|p| ![0, 4].contains(&p.index))));
        assert_eq!(
            (m.jobs[0].status.done, m.jobs[0].status.finished),
            (7, false)
        );
    }

    fn rect(args: &Args, s: &Session) -> XZBBox {
        crate::projection::ProjectionSpec::from_args(args)
            .transformer(&s.llbbox)
            .unwrap()
            .1
    }

    fn record(args: &Args, s: &Session, elevation: Option<ElevationAffine>) {
        let run = args.one_world_run.clone().unwrap();
        record_area(&run, &s.llbbox, &rect(args, s), elevation, None).unwrap();
    }

    /// A manifest the stock v3.4.0 exe wrote still opens, and one carrying
    /// piece records is still read by v3.4.0's own struct (unknown fields ignored).
    #[test]
    fn v340_manifest_still_opens() {
        #[derive(Deserialize)]
        struct V340Area {
            id: u32,
            min_x: i32,
            max_z: i32,
        }
        #[derive(Deserialize)]
        struct V340 {
            version: u32,
            next_area_id: u32,
            areas: Vec<V340Area>,
        }
        let text = include_str!("../tests/fixtures/one_world_v340.json");
        let mut m: Manifest = serde_json::from_str(text).unwrap();
        m.validate().unwrap();
        assert!(m.areas.iter().all(|a| a.piece.is_none()));
        let plain = serde_json::to_string(&m).unwrap();
        assert!(!plain.contains("piece"), "no piece key unless one is set");
        m.areas[0].piece = Some(PieceRef {
            job: "j".into(),
            index: 3,
        });
        let old: V340 = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!((old.version, old.next_area_id), (m.version, m.next_area_id));
        assert_eq!(
            (old.areas[0].id, old.areas[0].min_x),
            (m.areas[0].id, m.areas[0].min_x)
        );
        assert_eq!(old.areas[0].max_z, m.areas[0].max_z);
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    /// The place name and job records round-trip, and a manifest without them writes neither.
    #[test]
    fn place_name_and_jobs_round_trip() {
        let text = include_str!("../tests/fixtures/one_world_v340.json");
        let mut m: Manifest = serde_json::from_str(text).unwrap();
        assert!(m.place_name.is_none() && m.jobs.is_empty());
        let plain = serde_json::to_string(&m).unwrap();
        assert!(!plain.contains("place_name") && !plain.contains("jobs"));
        m.place_name = Some("Bucharest".into());
        m.jobs.push(JobRecord {
            name: "job".into(),
            settings: vec!["--unit-regions=8".into()],
            bbox: Some([48.1, 11.5, 48.2, 11.6]),
            status: JobStatus {
                done: 3,
                total: 9,
                finished: false,
                last_run_ms: 42,
            },
        });
        let back: Manifest = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        // A record missing its newer fields still reads.
        let bare: JobRecord = serde_json::from_str(r#"{"name":"j"}"#).unwrap();
        assert_eq!((bare.settings.len(), bare.status.total), (0, 0));
        assert_eq!(bare.bbox, None);
        let with: JobRecord = serde_json::from_str(r#"{"name":"j","bbox":[1,2,3,4]}"#).unwrap();
        assert_eq!(with.bbox, Some([1.0, 2.0, 3.0, 4.0]));
    }

    /// `record_job` keeps one record per job; a job folder from before records lists too.
    #[test]
    fn jobs_are_recorded_and_old_job_folders_list() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let flags = vec!["--seed=1".to_string()];
        let bb = [48.1, 11.5, 48.2, 11.6];
        record_job(&world, "a", Some(&flags), Some(bb), 0, 4, false).unwrap();
        record_job(&world, "a", None, None, 4, 4, true).unwrap();
        let old = world.join(JOBS_DIR).join("old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(
            old.join("job.json"),
            r#"{"area_base":1,"map_base":0,"slots":{"r0_0_s4":0,"r1_0_s4":1}}"#,
        )
        .unwrap();
        std::fs::write(old.join("done-r0_0_s4.json"), "{}").unwrap();
        let m = Manifest::load(&world).unwrap().unwrap();
        let jobs = job_records(&world, &m);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].settings, flags);
        assert_eq!((jobs[0].bbox, jobs[1].bbox), (Some(bb), None));
        assert_eq!((jobs[0].status.done, jobs[0].status.finished), (4, true));
        assert!(jobs[0].status.last_run_ms > 0);
        assert_eq!((jobs[1].name.as_str(), jobs[1].status.done), ("old", 1));
        assert_eq!((jobs[1].status.total, jobs[1].status.finished), (2, false));
    }

    #[test]
    fn a_new_world_gets_a_manifest_and_a_chunk_aligned_area() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("My One World");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &req, &mut args).unwrap();
        assert!(session.created);
        assert!(world.join("level.dat").is_file());
        assert!(!world.join("region").join("r.0.0.mca").exists());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert!(manifest.areas.is_empty());
        let area = rect(&args, &session);
        assert_eq!(area.min_x().rem_euclid(16), 0);
        assert_eq!((area.max_z() + 1).rem_euclid(16), 0);
        let run = args.one_world_run.as_ref().unwrap();
        assert_eq!(run.area_id, 1);
        assert!(!run.extending);
        assert_eq!(
            args.projection,
            crate::projection::ProjectionKind::WebMercator
        );
        assert_eq!(args.bbox.unwrap(), session.llbbox);
    }

    #[test]
    fn an_area_far_from_the_origin_counts_its_stretched_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let singapore = "1.290,103.840,1.300,103.860";
        let mut args = args_for(singapore, &[]);
        let first = prepare(&world, &LLBBox::from_str(singapore).unwrap(), &mut args).unwrap();
        drop(first);

        let oslo = "59.910,10.730,59.920,10.760";
        let mut args = args_for(oslo, &[]);
        let s = prepare(&world, &LLBBox::from_str(oslo).unwrap(), &mut args).unwrap();
        let ground = s.llbbox.area_km2() * args.scale * args.scale;
        let ratio = crate::world_utils::blocks_km2(&args, &s.llbbox) / ground;
        let want = (1.295f64.to_radians().cos() / 59.915f64.to_radians().cos()).powi(2);
        assert!((ratio / want - 1.0).abs() < 0.05, "{ratio} vs {want}");
    }

    #[test]
    fn recorded_areas_survive_and_a_second_run_extends() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &LLBBox::from_str(MUNICH).unwrap(), &mut args).unwrap();
        let run = args.one_world_run.clone().unwrap();
        let affine = run.elevation.expect("set when the world is created");
        let other = ElevationAffine {
            min_height_m: 500.0,
            ..affine
        };
        record(&args, &session, Some(other));
        drop(session);

        let east = "48.130,11.585,48.145,11.610";
        let mut args2 = args_for(east, &[]);
        let session2 = prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args2).unwrap();
        assert!(!session2.created);
        let run2 = args2.one_world_run.as_ref().unwrap();
        assert_eq!(run2.area_id, 2);
        assert!(run2.extending);
        assert_eq!(run2.elevation, Some(affine));
        assert_eq!(run2.origin_lat, run.origin_lat);
        assert_eq!(run2.origin_lon, run.origin_lon);
        assert_eq!(run2.replaced_chunks, 0, "nothing was written yet");
    }

    #[test]
    fn an_incompatible_setting_is_refused_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());

        let err = prepare(&world, &req, &mut args_for(MUNICH, &["--scale", "2"]))
            .err()
            .unwrap();
        assert!(err.contains("world scale 2.00"), "{err}");

        let err = prepare(&world, &req, &mut args_for(MUNICH, &["--mode", "geo-only"]))
            .err()
            .unwrap();
        assert!(err.contains("with terrain"), "{err}");
    }

    #[test]
    fn one_world_keeps_first_seed() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut first = args_for(MUNICH, &["--seed", "7"]);
        drop(prepare(&world, &req, &mut first).unwrap());
        assert_eq!(first.seed, Some(7));
        assert_eq!(Manifest::load(&world).unwrap().unwrap().seed, Some(7));
        // A later area asks for another, or none, and gets the world's.
        for later in [&["--seed", "9"][..], &[]] {
            let mut args = args_for(MUNICH, later);
            drop(prepare(&world, &req, &mut args).unwrap());
            assert_eq!(args.seed, Some(7), "{later:?}");
        }
        // A world made without a seed stays without one.
        let plain = dir.path().join("p");
        let mut none = args_for(MUNICH, &[]);
        drop(prepare(&plain, &req, &mut none).unwrap());
        let mut asks = args_for(MUNICH, &["--seed", "9"]);
        drop(prepare(&plain, &req, &mut asks).unwrap());
        assert_eq!(asks.seed, None);
    }

    #[test]
    fn a_new_world_refuses_the_vanilla_height() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--vanilla-height"]);
        let err = prepare(&world, &req, &mut args).err().unwrap();
        assert!(err.contains("--vanilla-height"), "{err}");
        assert!(!world.exists(), "the refused world was left behind");
    }

    #[test]
    fn a_new_world_has_room_for_all_of_earth() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        assert!(!args.disable_height_limit);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert!(args.disable_height_limit);
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert!(manifest.disable_height_limit);
        let e = manifest.elevation.unwrap();
        assert_eq!(e.y_for_metres(-430.0), -2014.0);
        assert_eq!(e.y_for_metres(520.0), -1064.0);
        assert!(e.y_for_metres(8849.0) <= 2016.0);
        // What the first run used is exactly what later runs read back.
        let run = args.one_world_run.as_ref().unwrap();
        assert_eq!(run.elevation, Some(e));
        assert_eq!(
            (run.origin_lat, run.origin_lon),
            (manifest.origin_lat, manifest.origin_lon)
        );

        let flat = dir.path().join("flat");
        let mut args = args_for(MUNICH, &["--mode", "geo-only"]);
        drop(prepare(&flat, &req, &mut args).unwrap());
        assert_eq!(Manifest::load(&flat).unwrap().unwrap().elevation, None);
    }

    #[test]
    fn the_height_multiplier_is_fixed_by_the_first_area() {
        let dir = tempfile::tempdir().unwrap();
        let req = LLBBox::from_str(MUNICH).unwrap();
        let real = dir.path().join("real");
        drop(prepare(&real, &req, &mut args_for(MUNICH, &[])).unwrap());
        let text = std::fs::read_to_string(Manifest::path_in(&real)).unwrap();
        assert!(!text.contains("height_multiplier"), "{text}");

        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &["--height-multiplier", "2"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(manifest.height_multiplier, 2.0);
        let e = manifest.elevation.unwrap();
        assert_eq!(e.blocks_per_meter, 2.0);
        assert_eq!(e.y_for_metres(520.0) - e.y_for_metres(-430.0), 1900.0);

        let mut args = args_for(MUNICH, &[]);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert_eq!(args.height_multiplier, 2.0);
        assert_eq!(args.one_world_run.unwrap().elevation, Some(e));
    }

    #[test]
    fn one_world_keeps_first_cave_seed() {
        let dir = tempfile::tempdir().unwrap();
        let req = LLBBox::from_str(MUNICH).unwrap();
        let plain = dir.path().join("plain");
        drop(prepare(&plain, &req, &mut args_for(MUNICH, &["--caves"])).unwrap());
        let text = std::fs::read_to_string(Manifest::path_in(&plain)).unwrap();
        assert!(
            !text.contains("cave_seed") && !text.contains("cave_datum_y"),
            "{text}"
        );

        let world = dir.path().join("w");
        let first = ["--caves", "--cave-seed", "5", "--cave-datum-y", "-64"];
        drop(prepare(&world, &req, &mut args_for(MUNICH, &first)).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(
            (manifest.cave_seed, manifest.cave_datum_y),
            (Some(5), Some(-64))
        );

        // A later area without the flags takes the stored values.
        let mut args = args_for(MUNICH, &["--caves"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert_eq!((args.cave_seed, args.cave_datum_y), (Some(5), Some(-64)));

        // A different value is refused.
        let mut args = args_for(MUNICH, &["--caves", "--cave-seed", "6"]);
        let err = prepare(&world, &req, &mut args).err().unwrap();
        assert!(err.contains("--cave-seed does not match"), "{err}");
    }

    #[test]
    fn a_one_world_is_always_void() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--world-type", "flat"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert_eq!(args.world_type, crate::args::WorldType::Void);
    }

    #[test]
    fn an_older_world_keeps_its_build_height() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let mut manifest = Manifest::load(&world).unwrap().unwrap();
        manifest.version = 2;
        manifest.disable_height_limit = false;
        manifest.elevation = None;
        manifest.save(&world).unwrap();

        let mut args = args_for(MUNICH, &["--disable-height-limit"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert!(!args.disable_height_limit);
        assert_eq!(args.one_world_run.unwrap().elevation, None);
        assert_eq!(Manifest::load(&world).unwrap().unwrap().version, 2);
    }

    #[test]
    fn a_locked_world_is_refused_and_the_session_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let session = prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap();
        let err = prepare(&world, &req, &mut args_for(MUNICH, &[]))
            .err()
            .unwrap();
        assert!(err.contains("open in Minecraft"), "{err}");
        drop(session);

        let held = SessionLock::acquire(&world).unwrap();
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &[])).is_err());
        drop(held);
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &[])).is_ok());
    }

    #[test]
    fn a_refused_first_run_leaves_no_folder_behind() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        // Wider than the world border at scale 4, refused after the lock is taken.
        let huge = "-10.0,-179.0,10.0,179.0";
        let err = prepare(
            &world,
            &LLBBox::from_str(huge).unwrap(),
            &mut args_for(huge, &["--scale", "4"]),
        )
        .err()
        .unwrap();
        assert!(err.contains("world border"), "{err}");
        assert!(!world.exists());
    }

    #[test]
    fn a_foreign_folder_is_not_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("level.dat"), b"x").unwrap();
        let err = prepare(
            &world,
            &LLBBox::from_str(MUNICH).unwrap(),
            &mut args_for(MUNICH, &[]),
        )
        .err()
        .unwrap();
        assert!(err.contains("not a One World"), "{err}");
    }

    #[test]
    fn refused_runs_create_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &["--rotation", "10"])).is_err());
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &["--bedrock"])).is_err());
        let polar = "86.0,10.0,86.1,10.1";
        assert!(prepare(
            &world,
            &LLBBox::from_str(polar).unwrap(),
            &mut args_for(polar, &[])
        )
        .is_err());
        assert!(!world.exists());
    }

    #[test]
    fn an_area_past_the_world_border_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--scale", "4"]);
        let session = prepare(&world, &req, &mut args).unwrap();
        record(&args, &session, None);
        drop(session);
        let far = "-33.88,151.19,-33.85,151.23";
        let err = prepare(
            &world,
            &LLBBox::from_str(far).unwrap(),
            &mut args_for(far, &["--scale", "4"]),
        )
        .err()
        .unwrap();
        assert!(err.contains("world border"), "{err}");
    }

    #[test]
    fn covered_areas_are_dropped_and_an_identical_rerun_replaces_itself() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        let s1 = prepare(&world, &req, &mut args).unwrap();
        record(&args, &s1, None);
        drop(s1);
        let mut again = args_for(MUNICH, &[]);
        let s1b = prepare(&world, &req, &mut again).unwrap();
        record(&again, &s1b, None);
        drop(s1b);
        let m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 1);
        assert_eq!(m.areas[0].id, 2);

        let big = "48.120,11.550,48.155,11.600";
        let mut args2 = args_for(big, &[]);
        let s2 = prepare(&world, &LLBBox::from_str(big).unwrap(), &mut args2).unwrap();
        record(&args2, &s2, None);
        let m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 1);
        assert_eq!(m.areas[0].id, 3);
        assert_eq!(m.extent().unwrap().min_x(), rect(&args2, &s2).min_x());
    }

    #[test]
    fn a_damaged_manifest_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let mut m = Manifest::load(&world).unwrap().unwrap();
        m.scale = 0.0;
        m.save(&world).unwrap();
        let err = Manifest::load(&world).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn preview_paths_stay_inside_the_preview_folder() {
        let w = Path::new("/w");
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/area-3.png"),
            Some(w.join(PREVIEW_DIR).join("area-3.png"))
        );
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/../level.dat"),
            None
        );
        assert_eq!(safe_preview_path(w, "../../etc/passwd"), None);
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/a/b.png"),
            None
        );
        assert_eq!(safe_preview_path(w, "arnis_one_world/previews/x.txt"), None);
    }

    #[test]
    fn existing_chunks_are_read_from_the_region_headers() {
        let dir = tempfile::tempdir().unwrap();
        let region = dir.path().join("region");
        std::fs::create_dir_all(&region).unwrap();
        let mut header = vec![0u8; 8192];
        // Chunks (0, 0) and (1, 0) of region (0, 0), and (31, 31) of region (-1, -1).
        header[0..4].copy_from_slice(&[0, 0, 2, 1]);
        header[4..8].copy_from_slice(&[0, 0, 3, 1]);
        std::fs::write(region.join("r.0.0.mca"), &header).unwrap();
        let mut other = vec![0u8; 8192];
        let i = 4 * (31 + 31 * 32);
        other[i..i + 4].copy_from_slice(&[0, 0, 2, 1]);
        std::fs::write(region.join("r.-1.-1.mca"), &other).unwrap();

        let rect = XZBBox::rect_from_min_max(-16, -16, 15, 15).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 2);
        let rect = XZBBox::rect_from_min_max(16, 0, 31, 15).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 1);
        let rect = XZBBox::rect_from_min_max(512, 512, 527, 527).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 0);
    }

    #[test]
    fn bounds_are_the_union_of_the_areas() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let mut m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.bounds(), None);
        let area = |min_lat, min_lon, max_lat, max_lon| GeneratedArea {
            id: 1,
            generated_at: 0,
            arnis_version: String::new(),
            min_x: 0,
            min_z: 0,
            max_x: 1,
            max_z: 1,
            min_lat,
            min_lon,
            max_lat,
            max_lon,
            preview: None,
            piece: None,
        };
        m.areas = vec![area(1.0, 2.0, 3.0, 4.0)];
        assert_eq!(m.bounds(), Some([1.0, 2.0, 3.0, 4.0]));
        m.areas.push(area(0.5, 3.0, 2.0, 5.0));
        assert_eq!(m.bounds(), Some([0.5, 2.0, 3.0, 5.0]));
    }

    #[test]
    fn a_manifest_without_areas_has_no_extent() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        drop(
            prepare(
                &world,
                &LLBBox::from_str(MUNICH).unwrap(),
                &mut args_for(MUNICH, &[]),
            )
            .unwrap(),
        );
        assert!(Manifest::load(&world).unwrap().unwrap().extent().is_none());
    }

    #[test]
    fn a_piece_builds_its_halo_but_never_past_the_selection() {
        let selection = XZBBox::rect_from_min_max(-320, -176, 319, 175).unwrap();
        let rect = XZBBox::rect_from_min_max(0, -176, 319, -1).unwrap();
        let b = piece_build_rect(&rect, &selection);
        let h = PIECE_HALO_BLOCKS;
        // Grown towards its neighbours, held at the selection's own edges.
        assert_eq!((b.min_x(), b.max_x()), (-h, 319));
        assert_eq!((b.min_z(), b.max_z()), (-176, -1 + h));
    }

    /// `--world-border` reads back from level.dat: around the area of a plain world, around
    /// every area of a One World.
    #[test]
    fn border_covers_all_areas() {
        fn border(world: &Path) -> std::collections::HashMap<String, f64> {
            let raw = std::fs::read(world.join("level.dat")).unwrap();
            let mut bytes = Vec::new();
            flate2::read::GzDecoder::new(raw.as_slice())
                .read_to_end(&mut bytes)
                .unwrap();
            let fastnbt::Value::Compound(root) = fastnbt::from_bytes(&bytes).unwrap() else {
                panic!("root");
            };
            let Some(fastnbt::Value::Compound(data)) = root.get("Data") else {
                panic!("Data");
            };
            data.iter()
                .filter_map(|(k, v)| match v {
                    fastnbt::Value::Double(d) if k.starts_with("Border") => Some((k.clone(), *d)),
                    _ => None,
                })
                .collect()
        }
        let dir = tempfile::tempdir().unwrap();
        let plain = crate::world_utils::create_new_world(dir.path()).unwrap();
        let plain = Path::new(&plain);
        apply_world_border(plain, &XZBBox::rect_from_min_max(-50, 10, 49, 49).unwrap());
        let b = border(plain);
        assert_eq!((b["BorderCenterX"], b["BorderCenterZ"]), (0.0, 30.0));
        assert_eq!((b["BorderSize"], b["BorderSizeLerpTarget"]), (100.0, 100.0));

        // A One World with two areas side by side: the union of both.
        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &LLBBox::from_str(MUNICH).unwrap(), &mut args).unwrap();
        let first = rect(&args, &session);
        record(&args, &session, None);
        drop(session);
        let east = "48.130,11.585,48.145,11.610";
        let mut args2 = args_for(east, &[]);
        let session2 = prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args2).unwrap();
        let second = rect(&args2, &session2);
        record(&args2, &session2, None);
        drop(session2);
        assert!(second.max_x() > first.max_x());
        apply_world_border(&world, &second);
        let (x0, z0) = (
            first.min_x().min(second.min_x()),
            first.min_z().min(second.min_z()),
        );
        let (x1, z1) = (
            first.max_x().max(second.max_x()),
            first.max_z().max(second.max_z()),
        );
        let b = border(&world);
        assert_eq!(b["BorderCenterX"], f64::from(x0 + x1 + 1) / 2.0);
        assert_eq!(b["BorderCenterZ"], f64::from(z0 + z1 + 1) / 2.0);
        assert_eq!(b["BorderSize"], f64::from((x1 + 1 - x0).max(z1 + 1 - z0)));
    }

    /// `--origin` pins block (0, 0) of a new world; an existing world keeps its own.
    #[test]
    fn origin_pins_a_new_world_only() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--origin", "48.1234567891,11.5432109876"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(
            (manifest.origin_lat, manifest.origin_lon),
            (48.1234568, 11.543211)
        );
        let frame = manifest.projection();
        assert_eq!(frame.x_for_lon(manifest.origin_lon), 0.0);
        assert_eq!(frame.z_for_lat(manifest.origin_lat), 0.0);

        let mut args = args_for(MUNICH, &["--origin", "-45,170"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let run = args.one_world_run.unwrap();
        assert_eq!(
            (run.origin_lat, run.origin_lon),
            (manifest.origin_lat, manifest.origin_lon)
        );
    }

    /// The same bbox under two origins is the same area in a shifted frame: equal size, every
    /// corner translated by the same amount (the planner sees the shifted frame too); an origin
    /// a whole number of chunks away moves the snapped area by exactly those chunks. This checks
    /// the frame, not generated blocks: block content under `--origin` is only smoke-tested.
    #[test]
    fn origin_shifts_the_frame() {
        let req = LLBBox::from_str(MUNICH).unwrap();
        let corners = |origin: &[&str]| {
            let args = args_for(MUNICH, origin);
            let proj = frame_for(Path::new("does-not-exist"), &req, &args).unwrap();
            let (rect, _) = snap_bbox_to_chunks(&proj, &req).unwrap();
            (rect.min_x(), rect.min_z(), rect.max_x(), rect.max_z())
        };
        let (a, b) = (corners(&[]), corners(&["--origin", "48.0,11.0"]));
        assert_eq!((a.2 - a.0, a.3 - a.1), (b.2 - b.0, b.3 - b.1), "same size");
        assert_ne!(a, b, "shifted");
        assert_eq!(
            (a.0 - b.0, a.1 - b.1),
            (a.2 - b.2, a.3 - b.3),
            "one translation"
        );

        // Origin B is block (48, 80) of frame A, so every block of B sits (48, 80) lower.
        let frame_a = frame_for(Path::new("does-not-exist"), &req, &args_for(MUNICH, &[])).unwrap();
        let origin = format!(
            "{:.9},{:.9}",
            frame_a.lat_for_z(80.0),
            frame_a.lon_for_x(48.0)
        );
        let c = corners(&["--origin", &origin]);
        assert_eq!(
            (a.0 - c.0, a.1 - c.1, a.2 - c.2, a.3 - c.3),
            (48, 80, 48, 80)
        );
    }
}
