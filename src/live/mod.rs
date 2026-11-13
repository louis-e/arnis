//! Live mode: the One World on a local vanilla server, generated around the players.
//!
//! The server fills any chunk it finds missing with void and keeps that for the session,
//! so players are never let near land that is not ready: a world border around the ready
//! land, a teleport back for the rest. Delivery never touches a file the server has open.

pub mod mojang;
pub mod plan;
pub mod rcon;
pub mod region;
pub mod server;
pub mod servers_dat;
pub mod tiles;

use crate::args::Args;
use crate::world_utils::{SessionLock, WorldLayout};
use fastnbt::Value;
use region::Bits;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_VIEW_DISTANCE: u32 = 12;
const JOURNAL: &str = "session.json";
const PENDING: &str = "pending";
const LIVE_PACK: &str = "arnis_live";
const BOSSBAR: &str = "arnis:live";
const POLL: Duration = Duration::from_millis(100);
/// Chunks of room the start area leaves around the players.
const START_ROOM: i32 = 8;
/// Largest border half-width, in chunks.
const MAX_BORDER_HALF: i32 = 64;
const VANILLA_BORDER: f64 = 59_999_968.0;
const MAX_TILE_TRIES: u32 = 3;
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(180);
/// Chunks the server loads around one it generates, as far out as structure starts.
const GENERATION_REACH: i32 = 13;

pub struct Config {
    pub saves_dir: PathBuf,
    pub world_name: String,
    /// Where play starts, as (lat, lon). Required for a world that does not exist yet.
    pub start: Option<(f64, f64)>,
    pub version: Option<String>,
    pub view_distance: u32,
    pub port: Option<u16>,
    pub messages: Messages,
}

/// What players read in the game, in the language of whoever started the session.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Messages {
    pub welcome: String,
    pub building: String,
    pub wall: String,
    pub moved_back: String,
    pub teleport_wait: String,
    pub teleport_failed: String,
}

impl Default for Messages {
    fn default() -> Self {
        Self {
            welcome: "Arnis keeps generating the land as you explore.".to_string(),
            building: "Arnis is generating the land ahead".to_string(),
            wall: "Arnis is still generating this way".to_string(),
            moved_back: "That land is not ready yet".to_string(),
            teleport_wait: "Arnis is generating the place you picked. You will be moved there when it is ready.".to_string(),
            teleport_failed: "Arnis could not generate the place you picked.".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    /// starting, running, stopping, stopped or failed.
    pub phase: String,
    /// What is happening, as a key the GUI words in its own language; `message` is English.
    pub stage: String,
    pub message: String,
    pub progress: Option<f64>,
    /// Start areas done and needed.
    pub step: Option<(u32, u32)>,
    /// A download running beside the start area.
    pub download: Option<Download>,
    pub address: Option<String>,
    pub version: Option<String>,
    pub world: String,
    pub players: Vec<PlayerSpot>,
    pub tiles_done: u32,
    pub tiles_running: u32,
    pub tiles_waiting: u32,
    pub tiles_failed: u32,
    /// A teleport asked for on the map is waiting for its land.
    pub teleport_pending: bool,
    /// The last teleport's land could not be generated; cleared after a while.
    pub teleport_failed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Download {
    /// server or java.
    pub what: String,
    pub progress: Option<f64>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlayerSpot {
    pub name: String,
    /// Where on Earth, while in the overworld.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// The player's face as a PNG data URL, once Mojang sent the skin.
    pub head: Option<String>,
}

/// What the session is asked to do from outside its thread.
#[derive(Default)]
struct Controls {
    stop: AtomicBool,
    teleport: Mutex<Option<(f64, f64)>>,
}

impl Controls {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct Reporter {
    status: Arc<Mutex<Status>>,
    notify: Arc<dyn Fn(&Status) + Send + Sync>,
    controls: Arc<Controls>,
}

impl Reporter {
    fn update(&self, f: impl FnOnce(&mut Status)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut s);
            if self.controls.stopping() && matches!(s.phase.as_str(), "starting" | "running") {
                s.phase = "stopping".to_string();
            }
            s.clone()
        };
        (self.notify)(&snapshot);
    }

    fn say(&self, stage: &str, message: impl Into<String>, progress: Option<f64>) {
        let message = message.into();
        println!("[live] {message}");
        self.update(|s| {
            s.stage = stage.to_string();
            s.message = message;
            s.progress = progress;
            s.step = None;
        });
    }
}

/// A live session on its own thread.
pub struct Session {
    controls: Arc<Controls>,
    world_dir: PathBuf,
    status: Arc<Mutex<Status>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    /// `args` are the generation settings for the tiles, and for the world if it is created.
    pub fn start(config: Config, args: Args, notify: Arc<dyn Fn(&Status) + Send + Sync>) -> Self {
        let controls = Arc::new(Controls::default());
        let world_dir = config.saves_dir.join(&config.world_name);
        let status = Arc::new(Mutex::new(Status {
            phase: "starting".to_string(),
            stage: "starting".to_string(),
            message: "Starting live mode".to_string(),
            world: config.world_name.clone(),
            ..Status::default()
        }));
        let reporter = Reporter {
            status: status.clone(),
            notify,
            controls: controls.clone(),
        };
        let thread = std::thread::spawn(move || {
            let result = run(config, args, &reporter);
            match result {
                Ok(()) => reporter.update(|s| {
                    s.phase = "stopped".to_string();
                    s.stage = "stopped".to_string();
                    s.message = "Live mode stopped. The world is saved.".to_string();
                    s.progress = None;
                    s.download = None;
                    s.players.clear();
                    s.teleport_pending = false;
                    s.teleport_failed = false;
                }),
                Err(e) => {
                    eprintln!("[live] {e}");
                    reporter.update(|s| {
                        s.phase = "failed".to_string();
                        s.stage = "failed".to_string();
                        s.message = e.clone();
                        s.error = Some(e);
                        s.progress = None;
                        s.download = None;
                        s.players.clear();
                        s.teleport_pending = false;
                        s.teleport_failed = false;
                    })
                }
            }
        });
        Self {
            controls,
            world_dir,
            status,
            thread: Some(thread),
        }
    }

    pub fn world_dir(&self) -> &Path {
        &self.world_dir
    }

    pub fn request_stop(&self) {
        self.controls.stop.store(true, Ordering::Release);
    }

    /// Moves the players to (lat, lon) once the land there is ready.
    pub fn teleport(&self, lat: f64, lon: f64) {
        *self
            .controls
            .teleport
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some((lat, lon));
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Stops the session and waits until the world is saved.
    pub fn stop_and_wait(mut self) {
        self.request_stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What a live session left that has to be put right, for when it did not end cleanly.
#[derive(Serialize, Deserialize, Default)]
struct Journal {
    /// The Arnis process running the session.
    #[serde(default)]
    owner: Option<u32>,
    rcon_port: Option<u16>,
    rcon_password: Option<String>,
    stubbed: Vec<(i32, i32)>,
    border_set: bool,
}

fn journal_path(world_dir: &Path) -> PathBuf {
    tiles::live_root(world_dir).join(JOURNAL)
}

impl Journal {
    fn load(world_dir: &Path) -> Option<Self> {
        let text = fs::read_to_string(journal_path(world_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self, world_dir: &Path) -> Result<(), String> {
        let path = journal_path(world_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::world_utils::replace_file_atomically(&path, text.as_bytes())?;
        server::keep_private(&path);
        Ok(())
    }
}

/// Puts right what an interrupted live session left: stubs, chunks that came too late,
/// the map counter and the world border. Only with the world closed.
pub fn recover(world_dir: &Path) -> Result<(), String> {
    let Some(journal) = Journal::load(world_dir) else {
        return Ok(());
    };
    println!("[live] Tidying up after the last live session...");
    // Before `finish`, which drops the journal that says the border needs it.
    if journal.border_set {
        leave_border(world_dir);
    }
    finish(world_dir, &journal).map_err(|e| format!("Failed to tidy up after live mode: {e}"))
}

/// The game version that last saved the world, from level.dat.
pub fn world_game_version(world_dir: &Path) -> Option<String> {
    let Ok(Value::Compound(root)) = read_gzip(&world_dir.join("level.dat")) else {
        return None;
    };
    let Some(Value::Compound(data)) = root.get("Data") else {
        return None;
    };
    let Some(Value::Compound(version)) = data.get("Version") else {
        return None;
    };
    match version.get("Name") {
        Some(Value::String(name)) => Some(name.clone()),
        _ => None,
    }
}

/// The version to run when none is picked: the last one played, unless it is older than
/// the world, which an older server would load as empty.
pub fn default_version(
    manifest: &mojang::Manifest,
    last_played: Option<String>,
    world: Option<String>,
) -> Option<String> {
    let world = world.filter(|w| manifest.release_time(w).is_some());
    let candidate = last_played
        .filter(|v| manifest.is_supported(v))
        .or_else(|| manifest.latest_release())?;
    match world {
        Some(w) if older_than(manifest, &candidate, &w) => Some(w),
        _ => Some(candidate),
    }
}

fn older_than(manifest: &mojang::Manifest, a: &str, b: &str) -> bool {
    matches!(
        (manifest.release_time(a), manifest.release_time(b)),
        (Some(x), Some(y)) if x < y
    )
}

fn region_dir_of(world_dir: &Path) -> PathBuf {
    WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("region")
}

/// The offline part of ending a session.
fn finish(world_dir: &Path, journal: &Journal) -> Result<(), String> {
    let region_dir = region_dir_of(world_dir);
    let mut stubbed: Vec<(i32, i32)> = journal.stubbed.clone();
    stubbed.sort_unstable();
    stubbed.dedup();
    for (rx, rz) in stubbed {
        let path = region::region_file(&region_dir, rx, rz);
        if path.exists() {
            region::normalize(&path, &region_dir, rx, rz)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            // Void the server wrote where a stub was read too early is generated next time.
            region::clear_void(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    let pending = tiles::live_root(world_dir).join(PENDING);
    if let Ok(entries) = fs::read_dir(&pending) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some((rx, rz)) = region::parse_region_name(&name) {
                fs::create_dir_all(&region_dir).map_err(|e| e.to_string())?;
                let live = region::region_file(&region_dir, rx, rz);
                let filled = region::fill_from(&live, &e.path(), &region_dir, rx, rz)
                    .map_err(|e| e.to_string())?;
                if filled > 0 {
                    println!("[live] Filled in {filled} chunks the server reached first in region {rx},{rz}.");
                }
            }
        }
    }
    let maps = WorldLayout::of(world_dir).maps_dir(world_dir);
    let highest = fs::read_dir(&maps)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| tiles::map_id_of(&e.file_name().to_string_lossy()))
        .max();
    if let Some(highest) = highest {
        if highest >= crate::map_item::next_map_id(&maps) {
            crate::map_item::set_map_counter(world_dir, highest)?;
        }
    }
    let root = tiles::live_root(world_dir);
    if let Ok(entries) = fs::read_dir(&root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("staging-") || name == PENDING {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
    let _ = fs::remove_file(journal_path(world_dir));
    Ok(())
}

/// The border a session leaves: vanilla, or a One World's own `--world-border` around every
/// area it holds now.
fn leave_border(world_dir: &Path) {
    reset_border_offline(world_dir);
    let manifest = crate::one_world::Manifest::load(world_dir).ok().flatten();
    if let Some(rect) = manifest.filter(|m| m.world_border).and_then(|m| m.extent()) {
        crate::one_world::apply_world_border(world_dir, &rect);
    }
}

/// The vanilla border, for a world a crashed session left with a small one.
fn reset_border_offline(world_dir: &Path) {
    let file = WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("data/minecraft/world_border.dat");
    if let Ok(Value::Compound(mut root)) = read_gzip(&file) {
        if let Some(Value::Compound(data)) = root.get_mut("data") {
            for (k, v) in [
                ("center_x", Value::Double(0.0)),
                ("center_z", Value::Double(0.0)),
                ("size", Value::Double(VANILLA_BORDER)),
                ("lerp_target", Value::Double(VANILLA_BORDER)),
                ("lerp_time", Value::Long(0)),
                ("damage_per_block", Value::Double(0.2)),
                ("safe_zone", Value::Double(5.0)),
                ("warning_blocks", Value::Int(5)),
                ("warning_time", Value::Int(15)),
            ] {
                data.insert(k.to_string(), v);
            }
            let _ = write_gzip(&file, &Value::Compound(root));
        }
    }
    let level = world_dir.join("level.dat");
    if let Ok(Value::Compound(mut root)) = read_gzip(&level) {
        if let Some(Value::Compound(data)) = root.get_mut("Data") {
            if data.contains_key("BorderSize") {
                for (k, v) in [
                    ("BorderCenterX", Value::Double(0.0)),
                    ("BorderCenterZ", Value::Double(0.0)),
                    ("BorderSize", Value::Double(VANILLA_BORDER)),
                    ("BorderSizeLerpTarget", Value::Double(VANILLA_BORDER)),
                    ("BorderSizeLerpTime", Value::Long(0)),
                    ("BorderDamagePerBlock", Value::Double(0.2)),
                    ("BorderSafeZone", Value::Double(5.0)),
                    ("BorderWarningBlocks", Value::Double(5.0)),
                    ("BorderWarningTime", Value::Double(15.0)),
                ] {
                    data.insert(k.to_string(), v);
                }
                let _ = write_gzip(&level, &Value::Compound(root));
            }
        }
    }
}

fn read_gzip(path: &Path) -> Result<Value, String> {
    use std::io::Read;
    let raw = fs::read(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(raw.as_slice())
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    fastnbt::from_bytes(&out).map_err(|e| e.to_string())
}

fn write_gzip(path: &Path, value: &Value) -> Result<(), String> {
    use std::io::Write;
    let bytes = fastnbt::to_bytes(value).map_err(|e| e.to_string())?;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&bytes).map_err(|e| e.to_string())?;
    let gz = enc.finish().map_err(|e| e.to_string())?;
    crate::world_utils::replace_file_atomically(path, &gz)
}

/// Game mode, difficulty and cheats from level.dat, in both the 1.21 and 26.x layouts.
fn world_rules(world_dir: &Path) -> (server::WorldRules, bool) {
    let mut rules = server::WorldRules {
        gamemode: "creative",
        difficulty: "peaceful",
        hardcore: false,
    };
    let mut cheats = true;
    if let Ok(Value::Compound(root)) = read_gzip(&world_dir.join("level.dat")) {
        if let Some(Value::Compound(d)) = root.get("Data") {
            if let Some(Value::Int(g)) = d.get("GameType") {
                rules.gamemode = match g {
                    0 => "survival",
                    2 => "adventure",
                    3 => "spectator",
                    _ => "creative",
                };
            }
            let level = |b: i64| match b {
                1 => "easy",
                2 => "normal",
                3 => "hard",
                _ => "peaceful",
            };
            if let Some(Value::Byte(b)) = d.get("Difficulty") {
                rules.difficulty = level(i64::from(*b));
            }
            if let Some(Value::Byte(h)) = d.get("hardcore") {
                rules.hardcore = *h != 0;
            }
            if let Some(Value::Compound(ds)) = d.get("difficulty_settings") {
                if let Some(Value::String(s)) = ds.get("difficulty") {
                    rules.difficulty = match s.as_str() {
                        "easy" => "easy",
                        "normal" => "normal",
                        "hard" => "hard",
                        _ => "peaceful",
                    };
                }
                if let Some(Value::Byte(h)) = ds.get("hardcore") {
                    rules.hardcore = *h != 0;
                }
            }
            if let Some(Value::Byte(c)) = d.get("allowCommands") {
                cheats = *c != 0;
            }
        }
    }
    (rules, cheats)
}

/// Overworld positions of everyone who played here, singleplayer included.
fn saved_positions(world_dir: &Path) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut read = |v: &Value| {
        let Value::Compound(p) = v else { return };
        let in_overworld = match p.get("Dimension") {
            Some(Value::String(d)) => d == "minecraft:overworld",
            Some(Value::Int(d)) => *d == 0,
            _ => true,
        };
        if let (true, Some(Value::List(pos))) = (in_overworld, p.get("Pos")) {
            if let (Some(Value::Double(x)), Some(Value::Double(z))) = (pos.first(), pos.get(2)) {
                out.push((*x, *z));
            }
        }
    };
    if let Ok(Value::Compound(root)) = read_gzip(&world_dir.join("level.dat")) {
        if let Some(Value::Compound(d)) = root.get("Data") {
            if let Some(player) = d.get("Player") {
                read(player);
            }
        }
    }
    for dir in ["players/data", "playerdata"] {
        if let Ok(entries) = fs::read_dir(world_dir.join(dir)) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "dat") {
                    if let Ok(v) = read_gzip(&e.path()) {
                        read(&v);
                    }
                }
            }
        }
    }
    out
}

fn install_live_pack(world_dir: &Path) -> Result<(), String> {
    let root = world_dir.join("datapacks").join(LIVE_PACK);
    let function = root.join("data/arnis_live/function");
    fs::create_dir_all(&function).map_err(|e| e.to_string())?;
    let meta = serde_json::json!({
        "pack": {
            "description": "Arnis live mode",
            "pack_format": 61,
            "supported_formats": { "min_inclusive": 61, "max_inclusive": 2147483647 },
            "min_format": [61, 0],
            "max_format": [2147483647, 2147483647]
        }
    });
    fs::write(
        root.join("pack.mcmeta"),
        serde_json::to_vec_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    // One function, so the centre and size change in the same tick.
    fs::write(
        function.join("border.mcfunction"),
        "$worldborder center $(x) $(z)\n$worldborder set $(size)\n",
    )
    .map_err(|e| e.to_string())?;
    crate::world_utils::enable_datapack_in_level_dat(world_dir, LIVE_PACK)
}

#[derive(Clone, Debug)]
enum Region {
    Complete,
    /// Present at the start, with `delivered` slots added through stubs.
    Partial {
        present: Bits,
        stubbed: bool,
        delivered: Bits,
    },
    /// The server reached it before it was ready; only `present` is usable this session.
    Burned {
        present: Bits,
    },
    /// Could not be generated this session.
    Blocked {
        present: Bits,
    },
}

#[derive(Default)]
struct Regions {
    map: HashMap<(i32, i32), Region>,
    failures: HashMap<(i32, i32), (u32, Instant)>,
    /// Complete regions already checked for void a game left in them.
    verified: HashSet<(i32, i32)>,
}

impl Regions {
    /// Reads which chunks exist. Runs with the server down, so it also clears what an earlier
    /// session's server filled with void in regions with gaps: those get generated again.
    fn scan(region_dir: &Path) -> Self {
        let mut out = Regions::default();
        if let Ok(entries) = fs::read_dir(region_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(r) = region::parse_region_name(&name) else {
                    continue;
                };
                if region::occupied(&e.path())
                    .ok()
                    .flatten()
                    .is_some_and(|b| !b.is_full())
                {
                    if let Ok(n) = region::clear_void(&e.path()) {
                        if n > 0 {
                            println!(
                                "[live] Region {},{}: {n} empty chunks will be generated again.",
                                r.0, r.1
                            );
                        }
                    }
                }
                if let Ok(Some(bits)) = region::occupied(&e.path()) {
                    let state = if bits.is_full() {
                        Region::Complete
                    } else {
                        Region::Partial {
                            present: bits,
                            stubbed: false,
                            delivered: Bits::default(),
                        }
                    };
                    out.map.insert(r, state);
                }
            }
        }
        out
    }

    fn ready(&self, cx: i32, cz: i32) -> bool {
        let i = region::slot_index(cx, cz);
        match self.map.get(&(cx >> 5, cz >> 5)) {
            None => false,
            Some(Region::Complete) => true,
            Some(Region::Partial {
                present, delivered, ..
            }) => present.get(i) || delivered.get(i),
            Some(Region::Burned { present } | Region::Blocked { present }) => present.get(i),
        }
    }

    fn needs_tile(&self, r: (i32, i32)) -> bool {
        match self.map.get(&r) {
            None => true,
            Some(Region::Partial {
                present, delivered, ..
            }) => !present.union(delivered).is_full(),
            _ => false,
        }
    }

    fn present(&self, r: (i32, i32)) -> Bits {
        match self.map.get(&r) {
            Some(Region::Complete) => Bits::all(),
            Some(Region::Partial { present, .. }) => *present,
            Some(Region::Burned { present } | Region::Blocked { present }) => *present,
            None => Bits::default(),
        }
    }

    fn waiting(&self, now: Instant, r: (i32, i32)) -> bool {
        self.failures.get(&r).is_some_and(|(_, at)| *at > now)
    }

    /// Failed every quick try; only the slow retry is left.
    fn gave_up(&self, r: (i32, i32)) -> bool {
        self.failures
            .get(&r)
            .is_some_and(|(tries, _)| *tries >= MAX_TILE_TRIES)
    }
}

/// Regions within `radius` chunks of the chunk under `(x, z)`.
fn regions_around(x: f64, z: f64, radius: i32) -> impl Iterator<Item = (i32, i32)> {
    let (cx, cz) = ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4);
    let (rx0, rx1) = ((cx - radius) >> 5, (cx + radius) >> 5);
    let (rz0, rz1) = ((cz - radius) >> 5, (cz + radius) >> 5);
    (rx0..=rx1).flat_map(move |rx| (rz0..=rz1).map(move |rz| (rx, rz)))
}

struct Player {
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vz: f64,
    seen: Instant,
    last_safe: Option<(f64, f64, f64)>,
    told: Option<Instant>,
}

/// Everything the running session works on.
struct Live<'a> {
    world_dir: PathBuf,
    region_dir: PathBuf,
    layout: WorldLayout,
    proj: crate::projection::WebMercatorProjection,
    regions: Regions,
    touched: HashSet<(i32, i32)>,
    journal: Journal,
    workers: tiles::Workers,
    /// Just under the build limit, where a player waits for the chunk below to load.
    top_y: i32,
    view: i32,
    margin: i32,
    reach: f64,
    reporter: &'a Reporter,
    done: u32,
    failed: u32,
}

impl Live<'_> {
    fn chunk_safe(&self, x: f64, z: f64) -> bool {
        let ready = |cx: i32, cz: i32| self.regions.ready(cx, cz);
        plan::chunk_is_safe(
            &ready,
            (x.floor() as i32) >> 4,
            (z.floor() as i32) >> 4,
            self.margin,
        )
    }

    fn touch(&mut self, x: f64, z: f64) {
        self.touched.extend(regions_around(x, z, self.margin));
    }

    /// Marks the regions the server may have read around a player: its view, and as far
    /// as generating reaches where part of that view is not ready.
    fn touch_player(&mut self, x: f64, z: f64) {
        let reads = self.view + 3;
        let ready = |cx: i32, cz: i32| self.regions.ready(cx, cz);
        let deep = !plan::chunk_is_safe(
            &ready,
            (x.floor() as i32) >> 4,
            (z.floor() as i32) >> 4,
            reads,
        );
        let radius = if deep {
            reads + GENERATION_REACH
        } else {
            reads
        };
        self.touched.extend(regions_around(x, z, radius));
    }

    /// Checks a complete region once for void a game wrote into it, which is then generated
    /// again. Only while nothing can have it open.
    fn verify(&mut self, r: (i32, i32)) -> bool {
        if !self.regions.verified.insert(r)
            || !matches!(self.regions.map.get(&r), Some(Region::Complete))
            || self.touched.contains(&r)
        {
            return false;
        }
        let path = region::region_file(&self.region_dir, r.0, r.1);
        match region::clear_void(&path) {
            Ok(n) if n > 0 => {
                println!(
                    "[live] Region {},{}: {n} empty chunks will be generated again.",
                    r.0, r.1
                );
                let present = region::occupied(&path).ok().flatten().unwrap_or_default();
                self.regions.map.insert(
                    r,
                    Region::Partial {
                        present,
                        stubbed: false,
                        delivered: Bits::default(),
                    },
                );
            }
            Ok(_) => {}
            Err(e) => eprintln!("[live] {}: {e}", path.display()),
        }
        true
    }

    /// The nearest chunk centre within `radius` chunks that a player may stand on.
    fn nearest_safe(&self, x: f64, z: f64, radius: i32) -> Option<(f64, f64)> {
        let (cx, cz) = ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4);
        let ready = |cx: i32, cz: i32| self.regions.ready(cx, cz);
        for r in 0..=radius {
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    if plan::chunk_is_safe(&ready, cx + dx, cz + dz, self.margin) {
                        return Some((
                            f64::from((cx + dx) * 16 + 8),
                            f64::from((cz + dz) * 16 + 8),
                        ));
                    }
                }
            }
        }
        None
    }

    /// Adds stubs to a region with gaps, unless the server may already have it open.
    fn prepare_partial(&mut self, r: (i32, i32)) -> Result<(), String> {
        let Some(Region::Partial {
            present,
            stubbed: false,
            ..
        }) = self.regions.map.get(&r).cloned()
        else {
            return Ok(());
        };
        if self.touched.contains(&r) {
            self.regions.map.insert(r, Region::Burned { present });
            return Ok(());
        }
        self.journal.stubbed.push(r);
        self.journal.save(&self.world_dir)?;
        let path = region::region_file(&self.region_dir, r.0, r.1);
        region::add_stubs(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.regions.map.insert(
            r,
            Region::Partial {
                present,
                stubbed: true,
                delivered: Bits::default(),
            },
        );
        Ok(())
    }

    /// Starts tiles for the most urgent regions while workers are free.
    fn dispatch(&mut self, wanted: &[(i32, i32)], server_up: bool) {
        let now = Instant::now();
        for &r in wanted {
            if self.workers.idle().is_none() {
                break;
            }
            if !self.regions.needs_tile(r)
                || self.workers.is_running(r)
                || self.regions.waiting(now, r)
            {
                continue;
            }
            if !self.regions.map.contains_key(&r) && server_up && self.touched.contains(&r) {
                self.regions.map.insert(
                    r,
                    Region::Burned {
                        present: Bits::default(),
                    },
                );
                continue;
            }
            // With the server down a region is filled directly; stubs are for while it runs.
            if server_up {
                if let Err(e) = self.prepare_partial(r) {
                    eprintln!("[live] {e}");
                    self.regions.map.insert(
                        r,
                        Region::Blocked {
                            present: self.regions.present(r),
                        },
                    );
                    continue;
                }
            }
            if !self.regions.needs_tile(r) {
                continue;
            }
            match tiles::region_bbox(&self.proj, r.0, r.1) {
                Ok((_, bbox)) => {
                    if let Some(w) = self.workers.idle() {
                        self.workers.dispatch(
                            w,
                            tiles::Job {
                                rx: r.0,
                                rz: r.1,
                                bbox,
                            },
                        );
                    }
                }
                Err(_) => {
                    self.regions.map.insert(
                        r,
                        Region::Blocked {
                            present: self.regions.present(r),
                        },
                    );
                }
            }
        }
    }

    /// Moves a finished tile into the world. `server_up` says whether files the server
    /// has open must be left alone.
    fn publish(&mut self, done: tiles::Done, server_up: bool) {
        let r = (done.rx, done.rz);
        let result = done
            .result
            .and_then(|staged| self.place(r, staged, server_up));
        self.workers.release(done.worker);
        match result {
            Ok(()) => {
                self.done += 1;
                if self.regions.gave_up(r) {
                    self.failed = self.failed.saturating_sub(1);
                }
                self.regions.failures.remove(&r);
                println!("[live] Region {},{} ready ({:.1}s)", r.0, r.1, done.secs);
            }
            Err(e) => {
                let tries = self.regions.failures.get(&r).map_or(0, |f| f.0) + 1;
                eprintln!("[live] Region {},{} failed (try {tries}): {e}", r.0, r.1);
                if tries == MAX_TILE_TRIES {
                    self.failed += 1;
                    let msg = format!("Some land could not be generated: {e}");
                    self.reporter.update(|s| s.error = Some(msg));
                }
                // Past the quick tries it is tried again now and then, so an outage ends.
                let wait = if tries >= MAX_TILE_TRIES {
                    RETRY_AFTER_FAILURE
                } else {
                    Duration::from_secs(5 * 4u64.pow(tries - 1))
                };
                self.regions
                    .failures
                    .insert(r, (tries, Instant::now() + wait));
            }
        }
    }

    fn place(
        &mut self,
        r: (i32, i32),
        staged: tiles::Staged,
        server_up: bool,
    ) -> Result<(), String> {
        let maps_dir = self.layout.maps_dir(&self.world_dir);
        fs::create_dir_all(&maps_dir).map_err(|e| e.to_string())?;
        for (id, src) in &staged.maps {
            let dst = maps_dir.join(tiles::map_file_name(self.layout, *id));
            if !region::link_no_replace(src, &dst).map_err(|e| e.to_string())? {
                let _ = fs::remove_file(src);
            }
        }
        fs::create_dir_all(&self.region_dir).map_err(|e| e.to_string())?;
        let live = region::region_file(&self.region_dir, r.0, r.1);
        match self.regions.map.get(&r).cloned() {
            None => {
                if server_up && self.touched.contains(&r) {
                    self.keep_pending(r, &staged.region)?;
                    self.regions.map.insert(
                        r,
                        Region::Burned {
                            present: Bits::default(),
                        },
                    );
                } else if region::link_no_replace(&staged.region, &live)
                    .map_err(|e| e.to_string())?
                {
                    self.regions.map.insert(r, Region::Complete);
                    self.regions.verified.insert(r);
                } else {
                    // The server made the file first: what is in it is its void.
                    self.keep_pending(r, &staged.region)?;
                    self.regions.map.insert(
                        r,
                        Region::Burned {
                            present: Bits::default(),
                        },
                    );
                }
            }
            Some(Region::Partial { .. }) if !server_up => {
                region::fill_from(&live, &staged.region, &self.region_dir, r.0, r.1)
                    .map_err(|e| e.to_string())?;
                let _ = fs::remove_file(&staged.region);
                self.regions.map.insert(r, Region::Complete);
                self.regions.verified.insert(r);
            }
            Some(Region::Partial {
                present,
                stubbed: true,
                mut delivered,
            }) => {
                // A stub the server read too early now holds its void; that chunk waits for
                // the end of the session.
                let stubs = region::external_slots(&live).map_err(|e| e.to_string())?;
                let slots = region::read_slots(&staged.region).map_err(|e| e.to_string())?;
                let mut missed = false;
                for (i, slot) in slots.iter().enumerate() {
                    if present.get(i) {
                        continue;
                    }
                    if let region::Slot::Inline { version: 2, data } = slot {
                        if !stubs.get(i) {
                            missed = true;
                            continue;
                        }
                        let (cx, cz) = (r.0 * 32 + (i % 32) as i32, r.1 * 32 + (i / 32) as i32);
                        region::write_mcc(&self.region_dir, cx, cz, data)
                            .map_err(|e| e.to_string())?;
                        delivered.set(i);
                    }
                }
                if missed {
                    self.keep_pending(r, &staged.region)?;
                } else {
                    let _ = fs::remove_file(&staged.region);
                }
                // What is still missing cannot come this session, so it is not built again.
                let usable = present.union(&delivered);
                let next = if usable.is_full() {
                    Region::Partial {
                        present,
                        stubbed: true,
                        delivered,
                    }
                } else {
                    Region::Burned { present: usable }
                };
                self.regions.map.insert(r, next);
            }
            Some(Region::Partial { present, .. }) => {
                self.keep_pending(r, &staged.region)?;
                self.regions.map.insert(r, Region::Burned { present });
            }
            Some(_) => {
                self.keep_pending(r, &staged.region)?;
            }
        }
        let (rect, ll) = tiles::region_bbox(&self.proj, r.0, r.1)?;
        crate::one_world::record_live_tile(&self.world_dir, &ll, &rect, staged.preview.as_deref())
    }

    /// Keeps a tile the server beat to its region, to be filled in after the session.
    fn keep_pending(&self, r: (i32, i32), staged: &Path) -> Result<(), String> {
        let dir = tiles::live_root(&self.world_dir).join(PENDING);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dst = dir.join(format!("r.{}.{}.mca", r.0, r.1));
        let _ = fs::remove_file(&dst);
        fs::rename(staged, &dst).map_err(|e| e.to_string())
    }

    fn counts(&self) -> (u32, u32) {
        (self.workers.running() as u32, self.done)
    }
}

/// Generates and publishes every region the start positions need, with the server down.
fn build_start_area(
    live: &mut Live,
    positions: &[(f64, f64)],
    controls: &Controls,
) -> Result<(), String> {
    let radius = live.margin + START_ROOM;
    let mut needed: Vec<(i32, i32)> = Vec::new();
    for &(x, z) in positions {
        for r in regions_around(x, z, radius) {
            live.verify(r);
            if live.regions.needs_tile(r) && !needed.contains(&r) {
                needed.push(r);
            }
        }
    }
    let total = needed.len();
    let mut shown = usize::MAX;
    loop {
        if controls.stopping() {
            return Err("Cancelled".to_string());
        }
        let remaining: Vec<(i32, i32)> = needed
            .iter()
            .copied()
            .filter(|r| live.regions.needs_tile(*r) && !live.regions.gave_up(*r))
            .collect();
        let finished = total - remaining.len();
        if total > 0 && finished != shown {
            shown = finished;
            live.reporter.say(
                "start_area",
                format!("Generating the land around your start: {finished} of {total} areas"),
                Some(finished as f64 / total as f64),
            );
            live.reporter
                .update(|s| s.step = Some((finished as u32, total as u32)));
        }
        if remaining.is_empty() {
            break;
        }
        live.dispatch(&remaining, false);
        if let Some(done) = live.workers.wait_recv(Duration::from_millis(200)) {
            live.publish(done, false);
        }
    }
    let failed = needed.iter().any(|r| live.regions.gave_up(*r));
    if failed {
        return Err("The land around the start could not be generated. Check your internet connection and try again.".to_string());
    }
    Ok(())
}

/// The world to play, made empty around `start` if it does not exist yet.
fn ensure_world(
    world_dir: &Path,
    args: &mut Args,
    start: Option<(f64, f64)>,
) -> Result<bool, String> {
    if crate::one_world::Manifest::load(world_dir)?.is_some() {
        return Ok(false);
    }
    let Some((lat, lon)) = start else {
        return Err(
            "Select where to start on the map first: this One World does not exist yet."
                .to_string(),
        );
    };
    crate::one_world::create_empty(world_dir, args, lat, lon)?;
    Ok(true)
}

struct Provisioned {
    java: PathBuf,
    jar: PathBuf,
    version: String,
}

fn provision(cfg: &Config, world_dir: &Path, reporter: &Reporter) -> Result<Provisioned, String> {
    let dir = server::server_dir();
    let manifest = mojang::Manifest::load(&dir)?;
    let game_dir = mojang::game_dir(&cfg.saves_dir);
    let world_version = world_game_version(world_dir);
    let version = match &cfg.version {
        Some(v) => v.clone(),
        None => default_version(
            &manifest,
            game_dir
                .as_deref()
                .and_then(|g| mojang::last_played(g, &manifest)),
            world_version.clone(),
        )
        .ok_or("Could not pick a Minecraft version")?,
    };
    if !manifest.is_supported(&version) {
        return Err(format!(
            "Minecraft {version} is too old for Arnis worlds; use {} or newer.",
            mojang::OLDEST_VERSION
        ));
    }
    if let Some(w) = world_version.filter(|w| older_than(&manifest, &version, w)) {
        return Err(format!(
            "This world was last opened in Minecraft {w}, and Minecraft {version} cannot load it. Choose {w} or newer."
        ));
    }
    reporter.update(|s| s.version = Some(version.clone()));
    let info = manifest.version_info(&version, &dir)?;
    let report = |what: &str, label: &str, f: Option<f64>| {
        let text = match f {
            Some(f) => format!("{label}: {:.0}%", f * 100.0),
            None => label.to_string(),
        };
        reporter.update(|s| {
            s.download = Some(Download {
                what: what.to_string(),
                progress: f,
                text,
            })
        });
        !reporter.controls.stopping()
    };
    let jar = mojang::server_jar(&info, &dir, &|label, f| report("server", label, f))?;
    let java = match mojang::find_java(game_dir.as_deref(), &info.java_component, &dir) {
        Some(j) => j,
        None => match mojang::download_java(&info.java_component, &dir, &|label, f| {
            report("java", label, f)
        }) {
            Ok(j) => j,
            Err(e) if e == "Cancelled" => return Err(e),
            Err(e) => mojang::system_java(info.java_major).ok_or_else(|| {
                format!(
                    "Could not get Java {} for the server ({e}). Play Minecraft {version} once in the launcher, or install Java {}, then try again.",
                    info.java_major, info.java_major
                )
            })?,
        },
    };
    reporter.update(|s| s.download = None);
    Ok(Provisioned { java, jar, version })
}

fn random_password() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 18];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Stops a server an earlier session left running, through the RCON details it saved.
pub(crate) fn stop_leftover_server(world_dir: &Path) {
    let Some(journal) = Journal::load(world_dir) else {
        return;
    };
    // A server whose Arnis still runs is that Arnis's to stop.
    if journal
        .owner
        .is_some_and(|pid| pid != std::process::id() && arnis_running(pid))
    {
        return;
    }
    let (Some(port), Some(pw)) = (journal.rcon_port, journal.rcon_password.as_deref()) else {
        return;
    };
    if let Ok(mut r) = rcon::Rcon::connect(port, pw) {
        println!("[live] Stopping the live server an earlier session left running...");
        let _ = r.cmd("stop");
        for _ in 0..300 {
            if !crate::world_utils::world_is_locked(world_dir) {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

fn arnis_running(pid: u32) -> bool {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .is_some_and(|p| p.name().to_string_lossy().to_lowercase().contains("arnis"))
}

/// The world's session.lock, for the parts that write it with the server down.
fn lock_world(world_dir: &Path) -> Result<SessionLock, String> {
    if crate::world_utils::world_is_locked(world_dir) {
        stop_leftover_server(world_dir);
        if crate::world_utils::world_is_locked(world_dir) {
            return Err(
                "The world is open in Minecraft. Leave the world (or close the game), then start live mode again."
                    .to_string(),
            );
        }
    }
    fs::create_dir_all(world_dir).map_err(|e| format!("{}: {e}", world_dir.display()))?;
    SessionLock::acquire(world_dir).map_err(|_| {
        "Arnis is still generating into this world. Start live mode when it is done.".to_string()
    })
}

fn run(cfg: Config, mut args: Args, reporter: &Reporter) -> Result<(), String> {
    let controls = &*reporter.controls;
    let world_dir = cfg.saves_dir.join(&cfg.world_name);
    reporter.say("checking", "Checking the world", None);
    let fresh = !world_dir.exists();
    let mut lock = Some(lock_world(&world_dir)?);
    recover(&world_dir)?;
    if let Err(e) = ensure_world(&world_dir, &mut args, cfg.start) {
        if fresh {
            drop(lock.take());
            let _ = fs::remove_dir(&world_dir);
        }
        return Err(e);
    }
    let manifest =
        crate::one_world::Manifest::load(&world_dir)?.ok_or("The One World manifest is missing")?;
    let proj = manifest.projection();
    let start_xz = cfg
        .start
        .map(|(lat, lon)| (proj.x_for_lon(lon), proj.z_for_lat(lat)));
    let mut positions: Vec<(f64, f64)> = start_xz.into_iter().collect();
    if let Some((x, z)) = crate::map_item::read_spawn_xz(&world_dir) {
        positions.push((f64::from(x) + 0.5, f64::from(z) + 0.5));
    }
    // The server loads where a player last was before anyone can move them.
    positions.extend(saved_positions(&world_dir));

    let journal = Journal {
        owner: Some(std::process::id()),
        ..Journal::default()
    };
    journal.save(&world_dir)?;

    let view = cfg.view_distance.clamp(4, 32) as i32;
    let margin = view + 4;
    let workers_n = if std::thread::available_parallelism().map_or(4, |n| n.get()) >= 6 {
        2
    } else {
        1
    };
    let flags = tiles::tile_flags(&args, &manifest);
    let next_map = crate::map_item::next_map_id(&WorldLayout::of(&world_dir).maps_dir(&world_dir));
    let workers = tiles::Workers::new(
        &world_dir,
        workers_n,
        flags,
        tiles::tile_env(&args),
        next_map,
    )?;
    let mut live = Live {
        layout: WorldLayout::of(&world_dir),
        region_dir: region_dir_of(&world_dir),
        world_dir: world_dir.clone(),
        proj,
        regions: Regions::default(),
        touched: HashSet::new(),
        journal,
        workers,
        top_y: if manifest.disable_height_limit {
            crate::world_utils::TALL_DATAPACK_FULL_RANGE.1 - 2
        } else {
            318
        },
        view,
        margin,
        reach: f64::from((margin + 2) * 16) + 512.0,
        reporter,
        done: 0,
        failed: 0,
    };
    live.regions = Regions::scan(&live.region_dir);

    let outcome = std::thread::scope(|s| -> Result<(), String> {
        let provisioning = s.spawn(|| {
            let provisioned = provision(&cfg, &world_dir, reporter);
            if provisioned.is_err() {
                // No server to start, so the start area need not wait either.
                reporter.update(|s| s.download = None);
                controls.stop.store(true, Ordering::Release);
            }
            provisioned
        });
        let built = build_start_area(&mut live, &positions, controls);
        if built.is_err() {
            // Nothing to serve: the downloads can stop too.
            controls.stop.store(true, Ordering::Release);
        }
        let provisioned = provisioning
            .join()
            .map_err(|_| "Preparing the server failed".to_string())?;
        // The error that stopped the other side is the one to report.
        let provisioned = match (built, provisioned) {
            (Ok(()), p) => p?,
            (Err(e), Err(p)) if e == "Cancelled" => return Err(p),
            (Err(e), _) => return Err(e),
        };
        if controls.stopping() {
            return Err("Cancelled".to_string());
        }
        // Only now, so a start that stops early leaves nobody above unbuilt land. The spawn
        // is only where players land the first time; Y is set when one arrives.
        if let Some((x, z)) = start_xz {
            crate::world_utils::set_spawn_in_level_dat(
                &world_dir,
                x.floor() as i32,
                live.top_y,
                z.floor() as i32,
            )?;
        }
        play(
            &cfg,
            &mut live,
            &mut lock,
            &positions,
            provisioned,
            start_xz,
        )
    });

    live.workers.kill_all();
    reporter.say("saving", "Saving the world", None);
    let journal = std::mem::take(&mut live.journal);
    if lock.is_none() {
        lock = relock(&world_dir);
    }
    let finished = match &lock {
        Some(_) => finish(&world_dir, &journal),
        // Tidied up the next time Arnis opens the world.
        None => Err(
            "The world was opened elsewhere before Arnis could tidy up. It is put right the next time live mode or One World uses it."
                .to_string(),
        ),
    };
    drop(lock);
    match (outcome, finished) {
        (Err(e), _) if e == "Cancelled" => Ok(()),
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// The lock again once the server has let go of it.
fn relock(world_dir: &Path) -> Option<SessionLock> {
    for _ in 0..50 {
        if let Ok(lock) = SessionLock::acquire(world_dir) {
            return Some(lock);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// The running part: server up, players in, tiles flowing.
fn play(
    cfg: &Config,
    live: &mut Live,
    lock: &mut Option<SessionLock>,
    positions: &[(f64, f64)],
    provisioned: Provisioned,
    start_xz: Option<(f64, f64)>,
) -> Result<(), String> {
    let world_dir = live.world_dir.clone();
    let reporter = live.reporter;
    let controls = &*reporter.controls;
    let dir = server::server_dir();
    if !server::eula_accepted(&dir) {
        return Err("Agree to the Minecraft EULA to run the live server.".to_string());
    }
    install_live_pack(&world_dir)?;
    let (rules, cheats) = world_rules(&world_dir);
    let port = server::free_port(cfg.port.or(Some(25565)));
    let rcon_port = server::free_port(None);
    let password = random_password();
    live.journal.rcon_port = Some(rcon_port);
    live.journal.rcon_password = Some(password.clone());
    live.journal.border_set = true;
    live.journal.save(&world_dir)?;

    // Stubs go in before the server can open the regions near the start.
    let near: Vec<(i32, i32)> = positions
        .iter()
        .flat_map(|&(x, z)| regions_around(x, z, live.margin + START_ROOM + 32))
        .collect();
    for r in near {
        live.prepare_partial(r)?;
    }

    let address = if port == 25565 {
        "localhost".to_string()
    } else {
        format!("localhost:{port}")
    };
    if let Some(game) = mojang::game_dir(&cfg.saves_dir) {
        if let Err(e) = servers_dat::ensure_entry(&game, &address) {
            eprintln!("[live] Could not add the server to the multiplayer list: {e}");
        }
    }
    reporter.update(|s| s.version = Some(provisioned.version.clone()));
    reporter.say(
        "server_starting",
        format!("Starting the Minecraft {} server", provisioned.version),
        None,
    );
    let launch = server::Launch {
        java: &provisioned.java,
        jar: &provisioned.jar,
        saves_dir: &cfg.saves_dir,
        world_name: &cfg.world_name,
        port,
        rcon_port,
        rcon_password: &password,
        view_distance: cfg.view_distance.clamp(4, 32),
        rules: &rules,
    };
    // The server takes the lock itself.
    drop(lock.take());
    let mut server = server::Server::start(&dir, &launch)?;
    let cancel = || controls.stopping();
    if let Err(e) = server.wait_ready(Duration::from_secs(900), &cancel) {
        server.stop(None, Duration::from_secs(60));
        leave_border(&world_dir);
        return Err(e);
    }
    // A 26.x server moves an older world into the new folder layout as it starts.
    live.layout = WorldLayout::of(&world_dir);
    live.region_dir = region_dir_of(&world_dir);
    for &(x, z) in positions {
        live.touch(x, z);
    }
    // The listener opens just after the server says it is ready.
    let mut connected = rcon::Rcon::connect(rcon_port, &password);
    for _ in 0..50 {
        if connected.is_ok() || !server.running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
        connected = rcon::Rcon::connect(rcon_port, &password);
    }
    let mut rcon = match connected {
        Ok(r) => r,
        Err(e) => {
            server.stop(None, Duration::from_secs(60));
            leave_border(&world_dir);
            return Err(format!("Could not talk to the server: {e}"));
        }
    };
    let result = session_loop(
        live,
        &mut server,
        &mut rcon,
        &cfg.messages,
        cheats,
        start_xz,
        positions,
        &address,
    );
    // `/stop` typed in the game ends the session like the Stop button.
    let result = match result {
        Err(e) => {
            for _ in 0..30 {
                if !server.running() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let tail = server.tail(60);
            let stopped =
                tail.contains("Stopping server") && !tail.contains("unexpected exception");
            if !server.running() && stopped {
                reporter.say(
                    "stopped_in_game",
                    "The server was stopped from inside the game.",
                    None,
                );
                Ok(())
            } else {
                Err(e)
            }
        }
        ok => ok,
    };

    reporter.update(|s| {
        s.phase = "stopping".to_string();
        s.players.clear();
    });
    reporter.say("stopping", "Stopping the server and saving the world", None);
    for c in [
        format!("bossbar remove {BOSSBAR}"),
        "worldborder center 0 0".to_string(),
        format!("worldborder set {VANILLA_BORDER}"),
        "worldborder damage amount 0.2".to_string(),
        "worldborder warning distance 5".to_string(),
    ] {
        let _ = rcon.cmd(&c);
    }
    server.stop(Some(&mut rcon), Duration::from_secs(120));
    // A server that went down on its own saved the border it had.
    leave_border(&world_dir);
    live.journal.border_set = false;
    result
}

/// A teleport asked for on the map, waiting for its land.
struct Teleport {
    x: f64,
    z: f64,
    since: Instant,
}

/// Puts `name` on the ground at (x, z). Where spreadplayers finds no dry spot, as on water,
/// the player waits above it until the chunk is loaded and then drops onto the top block.
fn land(
    rcon: &mut rcon::Rcon,
    name: &str,
    x: f64,
    z: f64,
    top_y: i32,
    landing: &mut HashMap<String, Instant>,
) -> Result<(), String> {
    let placed = rcon.cmd(&format!(
        "execute in minecraft:overworld run spreadplayers {x:.1} {z:.1} 0 1 false {name}"
    ))?;
    if !placed.contains("Spread") {
        rcon.cmd(&format!(
            "execute in minecraft:overworld run tp {name} {x:.1} {top_y} {z:.1}"
        ))?;
        landing.insert(name.to_string(), Instant::now());
    }
    Ok(())
}

/// How long a teleport waits for its land before it is given up.
const TELEPORT_PATIENCE: Duration = Duration::from_secs(300);

/// Moves the border to `sq`, through the live pack's function where it loaded.
fn set_border(
    rcon: &mut rcon::Rcon,
    use_function: &mut bool,
    sq: plan::Square,
) -> Result<(), String> {
    let (x, z, size) = sq.border();
    if *use_function {
        let reply = rcon.cmd(&format!(
            "function arnis_live:border {{x:{x:.1},z:{z:.1},size:{size:.1}}}"
        ))?;
        if !(reply.contains("Unknown") || reply.contains("Can't find")) {
            return Ok(());
        }
        *use_function = false;
    }
    rcon.cmd(&format!("worldborder center {x:.1} {z:.1}"))?;
    rcon.cmd(&format!("worldborder set {size:.1}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn session_loop(
    live: &mut Live,
    server: &mut server::Server,
    rcon: &mut rcon::Rcon,
    messages: &Messages,
    cheats: bool,
    start_xz: Option<(f64, f64)>,
    positions: &[(f64, f64)],
    address: &str,
) -> Result<(), String> {
    let fallback = positions.first().copied();
    let reporter = live.reporter;
    let controls = &*reporter.controls;
    for c in [
        "worldborder damage amount 0".to_string(),
        "worldborder warning distance 0".to_string(),
        format!(
            "bossbar add {BOSSBAR} {}",
            rcon::text(&messages.building, "white")
        ),
        format!("bossbar set {BOSSBAR} color green"),
        format!("bossbar set {BOSSBAR} visible false"),
    ] {
        rcon.cmd(&c)?;
    }
    reporter.update(|s| {
        s.phase = "running".to_string();
        s.address = Some(address.to_string());
    });
    reporter.say(
        "running",
        format!(
            "Ready! In Minecraft, open Multiplayer and join \"{}\" ({address}).",
            servers_dat::ENTRY_NAME
        ),
        None,
    );

    let mut players: HashMap<String, Player> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let mut joined: HashSet<String> = HashSet::new();
    let heads: Arc<Mutex<HashMap<String, Option<String>>>> = Arc::default();
    let mut border: Option<plan::Square> = None;
    let mut border_open = false;
    let mut use_function = true;
    let mut last_list = Instant::now() - Duration::from_secs(10);
    let mut last_status = Instant::now();
    let mut last_border = Instant::now();
    let mut published_since_border = false;
    let mut bossbar_shown = false;
    // Until someone is seen in the overworld, the border holds where players appear, so a
    // portal from the Nether cannot open onto unbuilt land.
    let starts: Vec<(i32, i32)> = positions
        .iter()
        .filter(|&&(x, z)| live.chunk_safe(x, z))
        .map(|&(x, z)| ((x.floor() as i32) >> 4, (z.floor() as i32) >> 4))
        .collect();
    if !starts.is_empty() {
        let ready = |cx: i32, cz: i32| live.regions.ready(cx, cz);
        if let Some(sq) = plan::safe_square(&ready, &starts, live.margin, MAX_BORDER_HALF) {
            set_border(rcon, &mut use_function, sq)?;
            border = Some(sq);
        }
    }
    let mut teleport: Option<Teleport> = None;
    let mut teleport_failed_at: Option<Instant> = None;
    let mut landing: HashMap<String, Instant> = HashMap::new();
    let pos_cmd = "execute as @a at @s if dimension minecraft:overworld run data get entity @s Pos";
    // Headless tests stand in for players with tagged entities they move and forceload around.
    let track_cmd = std::env::var("ARNIS_LIVE_TRACK").ok().map(|sel| {
        format!(
            "execute as {sel} at @s if dimension minecraft:overworld run data get entity @s Pos"
        )
    });

    while !controls.stopping() {
        let tick = Instant::now();
        if !server.running() {
            return Err(format!(
                "The server stopped. Last lines:\n{}",
                server.tail(8)
            ));
        }

        if tick.duration_since(last_list) >= Duration::from_secs(1) {
            last_list = tick;
            names = rcon::parse_list(&rcon.cmd("list")?);
            for name in &names {
                if joined.insert(name.clone()) {
                    {
                        let (heads, name) = (heads.clone(), name.clone());
                        std::thread::spawn(move || {
                            let head = mojang::player_head(&name);
                            heads
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(name, head);
                        });
                    }
                    if cheats {
                        let _ = rcon.cmd(&format!("op {name}"));
                    }
                    if let Some((x, z)) = start_xz {
                        land(rcon, name, x, z, live.top_y, &mut landing)?;
                        if !landing.contains_key(name) {
                            let _ = rcon.cmd(&format!("execute at {name} run setworldspawn ~ ~ ~"));
                        }
                    }
                    let _ = rcon.cmd(&format!(
                        "tellraw {name} {}",
                        rcon::text(&messages.welcome, "green")
                    ));
                }
            }
        }

        let now = Instant::now();
        let waiting_to_land: Vec<String> = landing.keys().cloned().collect();
        for name in waiting_to_land {
            let reply = rcon.cmd(&format!(
                "execute as {name} at @s positioned over motion_blocking run tp @s ~ ~ ~"
            ))?;
            let since = landing.get(&name).copied().unwrap_or(now);
            if reply.contains("Teleported") || now.duration_since(since) > Duration::from_secs(10) {
                landing.remove(&name);
                if start_xz.is_some() && joined.contains(&name) {
                    let _ = rcon.cmd(&format!("execute at {name} run setworldspawn ~ ~ ~"));
                }
            }
        }
        let mut seen = rcon::parse_positions(&rcon.cmd(pos_cmd)?);
        if let Some(cmd) = &track_cmd {
            seen.extend(rcon::parse_positions(&rcon.cmd(cmd)?));
        }
        for p in &seen {
            let entry = players.entry(p.name.clone()).or_insert(Player {
                x: p.x,
                y: p.y,
                z: p.z,
                vx: 0.0,
                vz: 0.0,
                seen: now,
                last_safe: None,
                told: None,
            });
            let dt = now.duration_since(entry.seen).as_secs_f64().max(0.05);
            let (dx, dz) = (p.x - entry.x, p.z - entry.z);
            let jumped = dx.hypot(dz) > 64.0;
            if jumped {
                entry.vx = 0.0;
                entry.vz = 0.0;
            } else {
                entry.vx = 0.7 * entry.vx + 0.3 * dx / dt;
                entry.vz = 0.7 * entry.vz + 0.3 * dz / dt;
            }
            entry.x = p.x;
            entry.y = p.y;
            entry.z = p.z;
            entry.seen = now;
        }
        players.retain(|_, p| now.duration_since(p.seen) < Duration::from_secs(2));
        let spots: Vec<(f64, f64)> = players.values().map(|p| (p.x, p.z)).collect();
        for (x, z) in spots {
            live.touch_player(x, z);
        }

        // Anyone on unready ground (a spectator, a /tp) goes back to where it was safe.
        for (name, p) in players.iter_mut() {
            if live.chunk_safe(p.x, p.z) {
                p.last_safe = Some((p.x, p.y, p.z));
                continue;
            }
            let cooled = p
                .told
                .is_none_or(|t| now.duration_since(t) > Duration::from_secs(3));
            if p.last_safe.is_none() {
                // Never seen on ready land this session, as after joining there: the nearest.
                if cooled && !landing.contains_key(name) {
                    p.told = Some(now);
                    if let Some((x, z)) = live.nearest_safe(p.x, p.z, 32).or(fallback) {
                        land(rcon, name, x, z, live.top_y, &mut landing)?;
                    }
                }
                continue;
            }
            if let Some((x, y, z)) = p.last_safe {
                println!("[live] {name} reached land that is not ready yet; moved back.");
                let _ = rcon.cmd(&format!("tp {name} {x:.2} {y:.2} {z:.2}"));
                p.x = x;
                p.z = z;
                p.vx = 0.0;
                p.vz = 0.0;
                if cooled {
                    p.told = Some(now);
                    let _ = rcon.cmd(&format!(
                        "title {name} actionbar {}",
                        rcon::text(&messages.moved_back, "gold")
                    ));
                }
            }
        }

        let asked = controls
            .teleport
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((lat, lon)) = asked {
            let (x, z) = (live.proj.x_for_lon(lon), live.proj.z_for_lat(lat));
            if tiles::region_bbox(&live.proj, (x.floor() as i32) >> 9, (z.floor() as i32) >> 9)
                .is_ok()
            {
                println!("[live] Teleport to {lat:.5}, {lon:.5} asked for.");
                teleport = Some(Teleport { x, z, since: now });
                teleport_failed_at = None;
                reporter.update(|s| {
                    s.teleport_pending = true;
                    s.teleport_failed = false;
                });
                if !live.chunk_safe(x, z) {
                    let _ = rcon.cmd(&format!(
                        "tellraw @a {}",
                        rcon::text(&messages.teleport_wait, "gold")
                    ));
                }
            }
        }
        if let Some(t) = &teleport {
            let blocked = regions_around(t.x, t.z, live.margin).any(|r| {
                live.regions.gave_up(r)
                    || matches!(
                        live.regions.map.get(&r),
                        Some(Region::Blocked { .. } | Region::Burned { .. })
                    )
            });
            if live.chunk_safe(t.x, t.z) {
                // The border goes first, or it would hold everyone where they were.
                let chunk = ((t.x.floor() as i32) >> 4, (t.z.floor() as i32) >> 4);
                let ready = |cx: i32, cz: i32| live.regions.ready(cx, cz);
                if let Some(sq) = plan::safe_square(&ready, &[chunk], live.margin, MAX_BORDER_HALF)
                {
                    set_border(rcon, &mut use_function, sq)?;
                    border = Some(sq);
                    border_open = false;
                }
                for name in &names {
                    land(rcon, name, t.x, t.z, live.top_y, &mut landing)?;
                }
                for p in players.values_mut() {
                    (p.x, p.z, p.vx, p.vz) = (t.x, t.z, 0.0, 0.0);
                    p.last_safe = None;
                }
                teleport = None;
                reporter.update(|s| s.teleport_pending = false);
            } else if blocked || now.duration_since(t.since) > TELEPORT_PATIENCE {
                teleport = None;
                teleport_failed_at = Some(now);
                reporter.update(|s| {
                    s.teleport_pending = false;
                    s.teleport_failed = true;
                });
                let _ = rcon.cmd(&format!(
                    "tellraw @a {}",
                    rcon::text(&messages.teleport_failed, "red")
                ));
            }
        }
        if teleport_failed_at.is_some_and(|t| now.duration_since(t) > Duration::from_secs(20)) {
            teleport_failed_at = None;
            reporter.update(|s| s.teleport_failed = false);
        }

        while let Some(done) = live.workers.try_recv() {
            live.publish(done, true);
            published_since_border = true;
        }
        let movers: Vec<plan::Mover> = players
            .values()
            .map(|p| plan::Mover {
                x: p.x,
                z: p.z,
                vx: p.vx,
                vz: p.vz,
            })
            .collect();
        // A teleport's land comes first.
        let mut wanted: Vec<(i32, i32)> = teleport
            .as_ref()
            .map(|t| {
                let at = plan::Mover {
                    x: t.x,
                    z: t.z,
                    vx: 0.0,
                    vz: 0.0,
                };
                plan::wanted_regions(&[at], live.reach, 15.0)
            })
            .unwrap_or_default();
        for r in plan::wanted_regions(&movers, live.reach, 15.0) {
            if !wanted.contains(&r) {
                wanted.push(r);
            }
        }
        // One check a tick: reading a whole region takes a moment.
        if let Some(&r) = wanted.iter().find(|r| {
            matches!(live.regions.map.get(r), Some(Region::Complete))
                && !live.regions.verified.contains(r)
        }) {
            live.verify(r);
        }
        // Stub what lies ahead early: once the server opens a region it is too late.
        let mut stubbed_now = 0;
        for &r in &wanted {
            if stubbed_now >= 4 {
                break;
            }
            if matches!(
                live.regions.map.get(&r),
                Some(Region::Partial { stubbed: false, .. })
            ) {
                live.prepare_partial(r)?;
                stubbed_now += 1;
            }
        }
        live.dispatch(&wanted, true);
        let waiting = wanted
            .iter()
            .filter(|r| live.regions.needs_tile(**r) && !live.workers.is_running(**r))
            .count();

        // The border: the largest safe square around everyone, kept until it needs to move.
        let chunks: Vec<(i32, i32)> = players
            .values()
            .filter(|p| live.chunk_safe(p.x, p.z))
            .map(|p| ((p.x.floor() as i32) >> 4, (p.z.floor() as i32) >> 4))
            .collect();
        if !chunks.is_empty() {
            let near_edge = border.is_none_or(|b| {
                chunks
                    .iter()
                    .any(|&(x, z)| !b.contains(x, z) || b.inset(x, z) < 6)
            });
            let grown =
                published_since_border && now.duration_since(last_border) > Duration::from_secs(2);
            if near_edge || grown || border_open {
                last_border = now;
                published_since_border = false;
                let ready = |cx: i32, cz: i32| live.regions.ready(cx, cz);
                match plan::safe_square(&ready, &chunks, live.margin, MAX_BORDER_HALF) {
                    Some(sq) if Some(sq) != border => {
                        set_border(rcon, &mut use_function, sq)?;
                        border = Some(sq);
                        border_open = false;
                    }
                    Some(_) => {}
                    None if !border_open => {
                        // Players too far apart for one square: the teleport back guards them.
                        rcon.cmd(&format!("worldborder set {VANILLA_BORDER}"))?;
                        border = None;
                        border_open = true;
                    }
                    None => {}
                }
            }
        }

        let (running, done) = live.counts();
        let building = running > 0 || waiting > 0;
        let near_wall = border.is_some_and(|b| chunks.iter().any(|&(x, z)| b.inset(x, z) < 8));
        let show = building && near_wall;
        if show != bossbar_shown {
            bossbar_shown = show;
            if show {
                rcon.cmd(&format!("bossbar set {BOSSBAR} players @a"))?;
            }
            rcon.cmd(&format!("bossbar set {BOSSBAR} visible {show}"))?;
        }
        if let Some(b) = border {
            for (name, p) in players.iter_mut() {
                let (cx, cz) = ((p.x.floor() as i32) >> 4, (p.z.floor() as i32) >> 4);
                if b.inset(cx, cz) == 0
                    && p.told
                        .is_none_or(|t| now.duration_since(t) > Duration::from_secs(4))
                {
                    p.told = Some(now);
                    let _ = rcon.cmd(&format!(
                        "title {name} actionbar {}",
                        rcon::text(&messages.wall, "gold")
                    ));
                }
            }
        }

        if now.duration_since(last_status) >= Duration::from_millis(500) {
            last_status = now;
            let failed = live.failed;
            let spots: Vec<PlayerSpot> = names
                .iter()
                .map(|name| {
                    let at = players.get(name);
                    PlayerSpot {
                        name: name.clone(),
                        lat: at.map(|p| live.proj.lat_for_z(p.z)),
                        lon: at.map(|p| live.proj.lon_for_x(p.x)),
                        head: heads
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get(name)
                            .cloned()
                            .flatten(),
                    }
                })
                .collect();
            reporter.update(|s| {
                s.tiles_done = done;
                s.tiles_running = running;
                s.tiles_waiting = waiting as u32;
                s.tiles_failed = failed;
                s.players = spots;
            });
        }
        let spent = tick.elapsed();
        if spent < POLL {
            std::thread::sleep(POLL - spent);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_gives_a_one_world_its_border_back() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;
        use clap::Parser;
        fn border_size(world: &Path) -> f64 {
            let Ok(Value::Compound(root)) = read_gzip(&world.join("level.dat")) else {
                panic!("level.dat");
            };
            match root.get("Data") {
                Some(Value::Compound(data)) => match data.get("BorderSize") {
                    Some(Value::Double(size)) => *size,
                    other => panic!("BorderSize: {other:?}"),
                },
                _ => panic!("Data"),
            }
        }
        let bbox = "48.130,11.560,48.145,11.590";
        let tile = XZBBox::rect_from_min_max(0, 0, 511, 511).unwrap();
        let played = XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for bordered in [true, false] {
            let world = dir.path().join(format!("w-{bordered}"));
            fs::create_dir_all(&world).unwrap();
            let mut args =
                crate::args::Args::parse_from(["arnis", "--output-dir", ".", "--bbox", bbox]);
            crate::one_world::create_empty(&world, &mut args, 48.13, 11.56).unwrap();
            let ll = LLBBox::from_str(bbox).unwrap();
            crate::one_world::record_live_tile(&world, &ll, &tile, None).unwrap();
            if bordered {
                crate::one_world::apply_world_border(&world, &tile);
            }
            // The session's own border, around where the players were.
            crate::world_utils::set_world_border(&world, &played).unwrap();
            leave_border(&world);
            let want = if bordered { 512.0 } else { VANILLA_BORDER };
            assert_eq!(border_size(&world), want);
        }
    }

    #[test]
    fn regions_around_cover_the_reach() {
        let got: Vec<_> = regions_around(10.0, 10.0, 2).collect();
        assert_eq!(got, vec![(-1, -1), (-1, 0), (0, -1), (0, 0)]);
        let far: Vec<_> = regions_around(256.0, 256.0, 2).collect();
        assert_eq!(far, vec![(0, 0)]);
    }

    #[test]
    fn readiness_follows_region_states() {
        let mut regions = Regions::default();
        regions.map.insert((0, 0), Region::Complete);
        let mut present = Bits::default();
        present.set(region::slot_index(33, 0));
        regions.map.insert(
            (1, 0),
            Region::Partial {
                present,
                stubbed: true,
                delivered: Bits::default(),
            },
        );
        assert!(regions.ready(5, 5));
        assert!(regions.ready(33, 0));
        assert!(!regions.ready(34, 0));
        assert!(!regions.ready(70, 0));
        assert!(regions.needs_tile((1, 0)));
        assert!(!regions.needs_tile((0, 0)));
        assert!(regions.needs_tile((5, 5)));
    }

    #[test]
    fn the_default_server_is_never_older_than_the_world() {
        let dir = tempfile::tempdir().unwrap();
        let list = serde_json::json!({
            "latest": { "release": "26.3" },
            "versions": [
                { "id": "26.3", "type": "release", "releaseTime": "2026-09-01T00:00:00+00:00" },
                { "id": "26.1", "type": "release", "releaseTime": "2026-03-01T00:00:00+00:00" },
                { "id": "1.21.5", "type": "release", "releaseTime": "2025-03-25T00:00:00+00:00" },
                { "id": "1.21.4", "type": "release", "releaseTime": "2024-12-03T00:00:00+00:00" }
            ]
        });
        fs::write(
            dir.path().join("version_manifest_v2.json"),
            list.to_string(),
        )
        .unwrap();
        let m = mojang::Manifest::load(dir.path()).unwrap();
        let pick = |played: Option<&str>, world: Option<&str>| {
            default_version(&m, played.map(String::from), world.map(String::from))
        };
        assert_eq!(pick(Some("1.21.5"), Some("26.1")).as_deref(), Some("26.1"));
        assert_eq!(pick(Some("26.3"), Some("26.1")).as_deref(), Some("26.3"));
        assert_eq!(pick(None, Some("1.21.4")).as_deref(), Some("26.3"));
        // A world from a version Mojang does not list (a snapshot gone) does not decide.
        assert_eq!(pick(Some("1.21.5"), Some("9.9")).as_deref(), Some("1.21.5"));
    }

    #[test]
    fn a_journal_is_finished_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path();
        let region_dir = world.join("region");
        fs::create_dir_all(&region_dir).unwrap();
        let mut slots = vec![region::Slot::Empty; region::SLOTS];
        slots[3] = region::Slot::External { version: 2 };
        region::write_slots(&region::region_file(&region_dir, 0, 0), &slots).unwrap();
        let journal = Journal {
            stubbed: vec![(0, 0)],
            ..Journal::default()
        };
        journal.save(world).unwrap();
        fs::create_dir_all(tiles::live_root(world).join("staging-0")).unwrap();
        recover(world).unwrap();
        assert!(!journal_path(world).exists());
        assert!(!tiles::live_root(world).join("staging-0").exists());
        let after = region::read_slots(&region::region_file(&region_dir, 0, 0)).unwrap();
        assert!(after.iter().all(|s| *s == region::Slot::Empty));
    }
}
