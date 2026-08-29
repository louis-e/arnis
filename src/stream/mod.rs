//! Stream mode: a loopback TCP server that generates Minecraft chunks on demand.
//!
//! A normal Arnis run generates a bounded area and writes a world folder. Stream mode instead
//! keeps Arnis resident: a client (today, a Minecraft Fabric mod) opens a socket on
//! `127.0.0.1`, sends a [`protocol::Hello`] describing how its world maps real elevation onto
//! block Y plus the real-world anchors it knows about, and then pulls 16x16 chunks as its
//! players move.
//!
//! Internally the server never generates a single chunk. It generates a **tile**
//! ([`StreamConfig::tile_size`], 512 blocks by default) plus a **margin**
//! ([`StreamConfig::margin`], 128 blocks) so that roads, rivers and building footprints
//! crossing the tile edge resolve with their full context, throws the margin away, caches the
//! inner tile and serves chunks out of that cache. First touch into a cold tile costs seconds;
//! the other 1023 chunks of that tile are cache hits.
//!
//! The wire protocol is specified normatively in `docs/stream-protocol.md`. This module owns the
//! server lifecycle: binding, the discovery file, the accept loop, the single generation worker,
//! and the [`StreamStatus`] block the GUI polls. Framing and message bodies live in
//! [`protocol`], per-connection behaviour in [`session`], the tile cache in [`tiles`], and
//! anchor placement in [`projection`].
//!
//! # "stream" here does NOT mean streaming to disk
//!
//! Arnis already has an unrelated feature spelled with the same word: `should_stream_to_disk`
//! in `data_processing.rs`, toggled by the `ARNIS_STREAM_TO_DISK` environment variable, which
//! evicts finished regions to disk during a normal generation so a large area does not have to
//! fit in RAM. That is a memory strategy inside one offline run. It has nothing to do with this
//! module, shares no code with it, and neither one enables or disables the other. If you got
//! here by grepping for "stream", check which of the two you actually wanted.

// Stream mode is reached from the GUI command layer and from the tests below; the binary's CLI
// path does not call it yet, so in a plain `cargo build` of the binary every entry point here
// reads as unused. Allowing dead code for the module beats sprinkling the attribute over each
// item, and it covers the sibling submodules too.
#![allow(dead_code)]

pub mod bench;
pub mod projection;
pub mod protocol;
pub mod session;
pub mod tiles;

use std::any::Any;
use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use colored::Colorize;
use fnv::FnvHashMap;
use rand::Rng;
use serde::{Deserialize, Serialize};

/// Default TCP port for stream mode. Arbitrary, in the ephemeral-adjacent range, and only ever
/// bound on loopback.
pub const DEFAULT_PORT: u16 = 41234;

/// Generation-bearing requests one client may have outstanding, advertised in `HelloOk`.
/// Beyond this the server answers `Error { code: "busy" }` instead of queueing without bound.
pub const MAX_IN_FLIGHT: u32 = 4;

/// Depth of the generation worker's queue. Comfortably above `MAX_IN_FLIGHT` per connection so
/// a full queue means genuine overload rather than ordinary bookkeeping.
const JOB_QUEUE_DEPTH: usize = 64;

/// How long the accept loop blocks before re-checking the shutdown flag.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long the generation worker waits for a job before re-checking the shutdown flag.
///
/// The worker cannot simply block until the channel closes: [`ServerContext`] holds a sender
/// too, and every live connection holds the context, so "all senders dropped" does not happen
/// while a client is still attached. The flag is the authority on shutdown; this is just how
/// often the worker looks at it.
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Lines kept in [`StreamStatus`]'s ring buffer. A stream server is long-lived, so an unbounded
/// log is a leak; the GUI only ever shows the tail anyway.
const LOG_CAPACITY: usize = 200;

/// Client sockets served at once.
///
/// Stream mode is a bridge to ONE Minecraft instance: the world bounds, the terrain floor and
/// the filler base are process globals tuned to the client's declared dimension, so a second
/// client with a different mapping would generate against the wrong world. The cap is not one,
/// though, because a client that reconnects (a world reload, a crashed mod) can legitimately
/// have its old session still draining an in-flight tile while the new socket arrives. Beyond
/// this the accept loop answers `HelloError { code: "busy" }` and closes, rather than spawning
/// an unbounded number of session threads for whatever opens the port.
const MAX_CONNECTIONS: usize = 4;

// -------------------------------------------------------------------------------------------
// Process-wide generation guard
// -------------------------------------------------------------------------------------------

/// Set while a generation owns the process.
///
/// The world floor, terrain floor and filler-chunk base (`world_editor::common`) are process
/// globals read from deep inside the block writers, and the terrain floor is derived from the
/// area's own elevation, so a second run would retune all three under the first one's feet.
/// The progress channel and world path are shared besides.
///
/// This lives here rather than in `gui.rs` because there are now two kinds of generation: the
/// GUI/CLI run that writes a world folder, and a stream-mode tile job. Both mutate the same
/// globals, so both have to take the same slot — one flag in one place, not two mechanisms that
/// do not know about each other.
static GENERATION_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Owns [`GENERATION_ACTIVE`] for the length of one generation and clears it on drop, including
/// on the early-return paths before a worker is spawned and on an unwind out of one.
pub(crate) struct GenerationSlot;

impl GenerationSlot {
    /// `None` when a generation is already running — a disk generation, or a stream tile job.
    pub(crate) fn acquire() -> Option<Self> {
        GENERATION_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for GenerationSlot {
    fn drop(&mut self) {
        GENERATION_ACTIVE.store(false, Ordering::Release);
    }
}

/// Live [`ProgressMute`] guards. Only the count is interesting, and only to the tests: the real
/// suppression lives in [`crate::progress`], which has no reader.
static PROGRESS_MUTES: AtomicU32 = AtomicU32::new(0);

/// Mutes the generation pipeline's GUI progress emits for as long as it is alive.
///
/// A tile job reuses the ordinary pipeline, which reports "Downloading data...", "Generating
/// area..." and so on to whatever window is listening. Without this guard every tile drives the
/// MAIN window's progress bar, status line and ETA even though the user started nothing, and a
/// pipeline `Error!` re-enables the Generate button under a generation that is still running.
/// `preview_3d` mutes its reuse of the pipeline the same way.
pub(crate) struct ProgressMute;

impl ProgressMute {
    pub(crate) fn new() -> Self {
        PROGRESS_MUTES.fetch_add(1, Ordering::Relaxed);
        crate::progress::set_progress_suppressed(true);
        Self
    }
}

impl Drop for ProgressMute {
    fn drop(&mut self) {
        crate::progress::set_progress_suppressed(false);
        let _ = PROGRESS_MUTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(1))
        });
    }
}

/// How many [`ProgressMute`] guards are currently held. For tests.
pub(crate) fn progress_mutes() -> u32 {
    PROGRESS_MUTES.load(Ordering::Relaxed)
}

/// Runtime settings for one stream-mode server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfig {
    /// TCP port on `127.0.0.1`. `0` means "let the OS pick a free one"; read the real port back
    /// with [`StreamHandle::port`].
    pub port: u16,
    /// Tile edge in blocks.
    pub tile_size: i32,
    /// Margin generated around each tile and then discarded, in blocks.
    pub margin: i32,
    /// Finished tiles kept in the cache.
    pub cache_tiles: usize,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            // Tile geometry and cache size belong to the tile layer, which also honours the
            // `ARNIS_STREAM_TILE_SIZE` / `_MARGIN` / `_CACHE_TILES` overrides. Duplicating the
            // numbers here would let the two drift apart silently.
            tile_size: tiles::tile_size(),
            margin: tiles::margin(),
            cache_tiles: tiles::cache_tiles(),
        }
    }
}

/// One registered anchor, flattened for display. Deliberately not
/// [`projection::Anchor`]: the GUI wants a serialisable value it can hold without pulling the
/// projection machinery in, and the status block is read from another thread.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorSummary {
    pub id: u32,
    pub lat: f64,
    pub lon: f64,
    pub mc_x: i32,
    pub mc_z: i32,
    pub radius_m: f64,
}

/// The parts of the status that need a lock: strings and lists, as opposed to counters.
#[derive(Debug)]
struct StreamStatusText {
    client_name: Option<String>,
    client_version: Option<String>,
    activity: String,
    anchors: Vec<AnchorSummary>,
    log: VecDeque<String>,
}

impl Default for StreamStatusText {
    fn default() -> Self {
        Self {
            client_name: None,
            client_version: None,
            activity: "Idle".to_string(),
            anchors: Vec::new(),
            log: VecDeque::new(),
        }
    }
}

/// Live server state, shared by every thread and polled by the GUI.
///
/// The counters are plain atomics so a status poll never contends with generation. Only the
/// text half takes a lock, and it is held for the length of a `push`/`clone` and no longer.
#[derive(Debug)]
pub struct StreamStatus {
    /// Port actually bound. Fixed for the life of the server.
    port: u16,
    /// `ChunkData` frames written to a client.
    pub chunks_served: AtomicU64,
    /// Tiles generated from scratch.
    pub tiles_generated: AtomicU64,
    /// Chunk/column requests answered from an already-generated tile.
    pub cache_hits: AtomicU64,
    /// Chunk/column requests that had to wait for a tile.
    pub cache_misses: AtomicU64,
    /// Generation-bearing requests currently outstanding across all connections.
    pub requests_in_flight: AtomicU64,
    /// Whether at least one client has completed a handshake and not yet disconnected.
    pub client_connected: AtomicBool,
    text: Mutex<StreamStatusText>,
}

/// A consistent-enough copy of [`StreamStatus`] for the GUI, in one serialisable value.
///
/// "Consistent enough" is honest: the counters are read one at a time, so a snapshot taken
/// mid-request can show a chunk counted as served before its in-flight slot is released. This is
/// a progress display, not an accounting ledger.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamStatusSnapshot {
    pub port: u16,
    pub client_connected: bool,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub activity: String,
    pub anchors: Vec<AnchorSummary>,
    pub log: Vec<String>,
    pub chunks_served: u64,
    pub tiles_generated: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub requests_in_flight: u64,
}

impl StreamStatus {
    fn new(port: u16) -> Self {
        Self {
            port,
            chunks_served: AtomicU64::new(0),
            tiles_generated: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            requests_in_flight: AtomicU64::new(0),
            client_connected: AtomicBool::new(false),
            text: Mutex::new(StreamStatusText::default()),
        }
    }

    /// A poisoned status lock must never take the server down: the data behind it is display
    /// text, and a panic elsewhere is already being reported through the log.
    fn text(&self) -> MutexGuard<'_, StreamStatusText> {
        self.text.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Port the server is listening on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Append one line to the bounded log, dropping the oldest line when full. Also echoed to
    /// the terminal, which is where a CLI run sees it.
    pub fn log(&self, line: impl Into<String>) {
        let line: String = line.into();
        println!("{} {}", "[stream]".cyan().bold(), line);
        let mut text = self.text();
        if text.log.len() >= LOG_CAPACITY {
            text.log.pop_front();
        }
        text.log.push_back(line);
    }

    /// Set the one-line "what is the server doing right now" string.
    pub fn set_activity(&self, activity: impl Into<String>) {
        self.text().activity = activity.into();
    }

    /// Record who is connected, after a successful handshake.
    pub fn set_client(&self, name: &str, version: &str) {
        let mut text = self.text();
        text.client_name = Some(name.to_string());
        text.client_version = Some(version.to_string());
    }

    /// Forget the connected client and go back to idle.
    pub fn clear_client(&self) {
        let mut text = self.text();
        text.client_name = None;
        text.client_version = None;
        text.anchors.clear();
        text.activity = "Idle".to_string();
    }

    /// Replace the displayed anchor list.
    pub fn set_anchors(&self, anchors: Vec<AnchorSummary>) {
        self.text().anchors = anchors;
    }

    /// Copy everything the GUI displays out in one go.
    pub fn snapshot(&self) -> StreamStatusSnapshot {
        let text = self.text();
        StreamStatusSnapshot {
            port: self.port,
            client_connected: self.client_connected.load(Ordering::Relaxed),
            client_name: text.client_name.clone(),
            client_version: text.client_version.clone(),
            activity: text.activity.clone(),
            anchors: text.anchors.clone(),
            log: text.log.iter().cloned().collect(),
            chunks_served: self.chunks_served.load(Ordering::Relaxed),
            tiles_generated: self.tiles_generated.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            requests_in_flight: self.requests_in_flight.load(Ordering::Relaxed),
        }
    }
}

/// One unit of work for the single generation worker.
///
/// `run` does everything, including writing the reply: only the connection that created the job
/// knows which response channel and request id the answer belongs to. `on_panic` exists because
/// the generation pipeline is not panic-free (`expect` inside rayon closures, release-mode
/// overflow checks), and a client waiting on a request that panicked must get a terminal
/// message rather than hang forever.
pub(crate) struct Job {
    /// Short label for the status line and the log, e.g. `"chunk 3,-7"`.
    pub(crate) label: String,
    /// The work itself.
    pub(crate) run: Box<dyn FnOnce() + Send + 'static>,
    /// Called with the panic message when `run` unwinds; reports `generation_failed`.
    pub(crate) on_panic: Box<dyn FnOnce(String) + Send + 'static>,
    /// Called *instead of* `run` when a normal generation owns the process, so the client gets
    /// a terminal `busy` rather than waiting on a job that will never run.
    pub(crate) on_busy: Box<dyn FnOnce() + Send + 'static>,
}

/// Why a job could not be enqueued.
///
/// The two cases must not be conflated: a full queue is ordinary back-pressure the client should
/// retry, while a closed queue means the worker is gone and retrying will never help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubmitError {
    /// The worker's queue is at [`JOB_QUEUE_DEPTH`].
    Full,
    /// The worker is gone: the server is shutting down, or its thread died.
    Closed,
}

impl SubmitError {
    /// The wire error code a waiter is answered with.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            SubmitError::Full => "busy",
            SubmitError::Closed => "generation_failed",
        }
    }

    /// The wire error message a waiter is answered with.
    pub(crate) fn message(&self) -> &'static str {
        match self {
            SubmitError::Full => "the generation queue is full; retry this request shortly",
            SubmitError::Closed => {
                "the generation worker is no longer running; stream mode is shutting down"
            }
        }
    }
}

/// Submission end of the generation worker's queue.
#[derive(Clone)]
pub(crate) struct JobQueue {
    tx: SyncSender<Job>,
}

impl JobQueue {
    /// Enqueue work a client is waiting on.
    ///
    /// Never blocks: the caller is a connection's reader thread, which must stay responsive to
    /// `Ping` and `Cancel` while generation is backed up. A full queue is reported to the client
    /// as `Error { code: "busy" }`, exactly like exceeding `maxInFlight`.
    ///
    /// A *disconnected* queue is reported too, and that is the whole point of the `Result`: the
    /// send destroys the job, so nothing else will ever answer the request it carried. Treating
    /// that as a successful enqueue leaves the client waiting for a terminal frame that cannot
    /// come, with its in-flight slot held for the life of the connection.
    pub(crate) fn submit(&self, job: Job) -> Result<(), SubmitError> {
        match self.tx.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SubmitError::Full),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::Closed),
        }
    }

    /// Enqueue a `Prefetch` hint, which is dropped when the queue is full.
    ///
    /// The protocol says explicitly that the server may ignore prefetch hints; dropping them
    /// under load is the correct behaviour, not a degradation, because the queue is full of
    /// requests a client is actually blocked on.
    pub(crate) fn submit_hint(&self, job: Job) -> Result<(), SubmitError> {
        self.submit(job)
    }
}

/// Everything a connection needs from the server it belongs to.
pub(crate) struct ServerContext {
    pub(crate) config: StreamConfig,
    /// The token a client must echo in `Hello`. See the security note in `docs/stream-protocol.md`:
    /// this stops cross-talk between processes, it is not an authorisation boundary.
    pub(crate) session_token: String,
    pub(crate) status: Arc<StreamStatus>,
    pub(crate) jobs: JobQueue,
    /// Set by [`StreamHandle::stop`]; polled by the accept loop and checked by each connection.
    pub(crate) shutdown: Arc<AtomicBool>,
    /// Live client sockets, so shutdown can unblock readers that are parked in `read`.
    ///
    /// A read timeout would be the obvious alternative and is wrong here: a timeout that fires
    /// *between* frames is harmless, but one that fires in the middle of a frame has already
    /// consumed part of the header or payload, and those bytes cannot be pushed back — the
    /// stream desynchronises. `shutdown(Both)` makes a blocked `read` return immediately without
    /// ever splitting a frame.
    connections: Mutex<FnvHashMap<u64, TcpStream>>,
    next_connection_id: AtomicU64,
    /// Connections that have completed a handshake and not yet finished.
    ///
    /// The shared client status belongs to the *set* of sessions, not to whichever one happens
    /// to end first: a port scan or a failed handshake must not wipe the connected client's name
    /// out from under it.
    pub(crate) live_sessions: AtomicUsize,
}

impl ServerContext {
    /// Register a connection's socket for shutdown, returning the token that removes it again.
    ///
    /// The shutdown flag is read while holding the very lock [`ServerContext::shutdown_connections`]
    /// drains under, and `stop` stores the flag (SeqCst) before taking that lock. So either this
    /// wins the lock and the drain closes the socket, or the drain ran first and this sees the
    /// flag and closes the socket itself. Without the check, a connection accepted in that window
    /// registers into an already-drained map: nothing will ever wake its reader out of the
    /// untimed `read`, and the accept loop's join at the end of [`accept_loop`] blocks forever.
    pub(crate) fn register_connection(&self, stream: &TcpStream) -> u64 {
        let id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let mut connections = self.connections.lock().unwrap_or_else(|e| e.into_inner());
        if self.shutdown.load(Ordering::SeqCst) {
            let _ = stream.shutdown(Shutdown::Both);
            return id;
        }
        match stream.try_clone() {
            Ok(clone) => {
                connections.insert(id, clone);
            }
            // An unregistered socket is unwakeable for exactly the same reason, so close it now
            // rather than leave a reader parked on it until the client happens to speak.
            Err(e) => {
                drop(connections);
                self.status.log(format!(
                    "Could not register a client socket for shutdown: {e}"
                ));
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        id
    }

    /// Forget a connection that has finished on its own.
    pub(crate) fn unregister_connection(&self, id: u64) {
        self.connections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    /// Half-close every live client socket so its reader wakes up and sees the shutdown flag.
    fn shutdown_connections(&self) {
        let mut connections = self.connections.lock().unwrap_or_else(|e| e.into_inner());
        for (_, stream) in connections.drain() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// A running stream-mode server.
///
/// Dropping the handle does **not** stop the server — the threads are detached from it and the
/// discovery file would be left behind. Call [`StreamHandle::stop`].
pub struct StreamHandle {
    port: u16,
    ctx: Arc<ServerContext>,
    accept_thread: Option<JoinHandle<()>>,
    worker_thread: Option<JoinHandle<()>>,
    discovery_path: PathBuf,
}

impl StreamHandle {
    /// Port the server actually bound. Differs from the requested port when `0` was passed.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The live status block, safe to read from any thread.
    pub fn status(&self) -> Arc<StreamStatus> {
        Arc::clone(&self.ctx.status)
    }

    /// Shut the server down and clean up: stop accepting, let the connections notice, drain the
    /// job queue without running what is left, join the threads and delete the discovery file.
    ///
    /// A tile job already in progress runs to completion. The pipeline has no interrupt point
    /// inside a stage, so the alternative would be leaving a rayon fan-out running over data the
    /// worker had dropped.
    pub fn stop(mut self) {
        self.ctx.shutdown.store(true, Ordering::SeqCst);
        // Wake every reader parked in `read`. Without this a connected but idle client would
        // keep its session thread alive until it happened to send something, and the accept
        // loop joins those threads.
        self.ctx.shutdown_connections();
        // Accept first, then the worker. The accept loop joins the connection threads, and a
        // connection thread joins its writer, which only finishes once the jobs still holding a
        // response channel have been dropped -- which is the worker's doing. Joining the worker
        // first would therefore be the same wait in a worse order, not a shorter one.
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.worker_thread.take() {
            let _ = handle.join();
        }
        if let Err(e) = std::fs::remove_file(&self.discovery_path) {
            if e.kind() != ErrorKind::NotFound {
                eprintln!(
                    "{} could not remove the stream discovery file at {}: {e}",
                    "Warning:".yellow().bold(),
                    self.discovery_path.display()
                );
            }
        }
        self.ctx
            .status
            .client_connected
            .store(false, Ordering::Relaxed);
        self.ctx.status.set_activity("Stopped");
        self.ctx.status.log("Stream mode stopped.");
    }
}

/// Contents of `<config dir>/arnis/stream.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryFile {
    pub port: u16,
    pub pid: u32,
    pub protocol_version: u32,
    pub arnis_version: String,
    pub session_token: String,
}

/// Path of the discovery file: the platform config directory plus `arnis/stream.json`.
pub fn discovery_path() -> Result<PathBuf, String> {
    dirs::config_dir()
        .map(|dir| dir.join("arnis").join("stream.json"))
        .ok_or_else(|| {
            "Could not determine this system's configuration directory, so stream mode has \
             nowhere to publish its port."
                .to_string()
        })
}

/// Whether a process with this id currently exists.
///
/// Used only to tell a crashed instance's leftover discovery file from a live instance's. A pid
/// can be recycled, which would make us refuse to start when we could have taken over; that is
/// the safe direction to be wrong in, and the user can delete the file.
fn pid_is_alive(pid: u32) -> bool {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some()
}

/// Publish the port and token so a client can find us.
///
/// A crashed run leaves the file behind. That is the normal recovery path, not an error: if the
/// recorded pid is gone (or is our own, e.g. a restart inside one process) the file is
/// overwritten silently. Only a *live* foreign pid means a second instance is already serving.
fn write_discovery_file(port: u16, session_token: &str) -> Result<PathBuf, String> {
    let path = discovery_path()?;

    if let Ok(existing) = std::fs::read_to_string(&path) {
        if let Ok(previous) = serde_json::from_str::<DiscoveryFile>(&existing) {
            if previous.pid != std::process::id() && pid_is_alive(previous.pid) {
                return Err(format!(
                    "Another Arnis instance is already running stream mode (on port {}). \
                     Stop it first, or close the other Arnis window.",
                    previous.port
                ));
            }
        }
    }

    let file = DiscoveryFile {
        port,
        pid: std::process::id(),
        protocol_version: protocol::PROTOCOL_VERSION,
        arnis_version: env!("CARGO_PKG_VERSION").to_string(),
        session_token: session_token.to_string(),
    };
    let body = serde_json::to_string_pretty(&file)
        .map_err(|e| format!("Could not encode the stream discovery file: {e}"))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Could not create the Arnis configuration folder at {}: {e}",
                parent.display()
            )
        })?;
    }
    std::fs::write(&path, body).map_err(|e| {
        format!(
            "Could not write the stream discovery file at {}: {e}",
            path.display()
        )
    })?;
    Ok(path)
}

/// 32 lowercase hex characters, i.e. 128 random bits, regenerated on every start.
fn generate_session_token() -> String {
    format!("{:032x}", rand::rng().random::<u128>())
}

/// Turn a bind failure into a sentence the person in front of the GUI can act on.
///
/// An `io::Error` debug print ("Os { code: 48, kind: AddrInUse, .. }") tells a non-technical
/// user nothing, and stream mode's whole audience is people running a game mod.
fn describe_bind_error(port: u16, err: &std::io::Error) -> String {
    match err.kind() {
        ErrorKind::AddrInUse => {
            format!("Port {port} is already in use. Try a different port.")
        }
        ErrorKind::PermissionDenied => {
            format!("Permission denied binding port {port}. Try a port above 1024.")
        }
        _ => format!("Could not start the stream server on port {port}: {err}"),
    }
}

/// Start stream mode.
///
/// Binds loopback only, publishes the discovery file, and spawns the accept thread and the
/// single generation worker. Returns as soon as the port is live; the first client can connect
/// immediately.
pub fn start(config: StreamConfig) -> Result<StreamHandle, String> {
    // LOOPBACK ONLY, and deliberately not configurable. Binding 0.0.0.0 would pop the Windows
    // Firewall dialog on first run, which is a terrible first impression for a local mod bridge,
    // and would expose an unauthenticated generation server to the network. A user who genuinely
    // wants remote access can forward the port themselves.
    let listener = TcpListener::bind(("127.0.0.1", config.port))
        .map_err(|e| describe_bind_error(config.port, &e))?;

    // Port 0 means "pick a free one", so the real port has to come back from the socket.
    let port = listener
        .local_addr()
        .map_err(|e| format!("Could not read back the stream server's port: {e}"))?
        .port();

    // The accept loop polls instead of blocking so that `stop` needs nothing but a flag. The
    // alternative -- waking the loop by connecting to ourselves -- races with a real client
    // arriving at the same instant and has to handle its own connect failing, for no benefit:
    // a 50 ms poll on an idle loopback listener costs nothing measurable.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Could not configure the stream server's socket: {e}"))?;

    let session_token = generate_session_token();
    let discovery_path = write_discovery_file(port, &session_token)?;

    let status = Arc::new(StreamStatus::new(port));
    let shutdown = Arc::new(AtomicBool::new(false));
    let (tx, rx) = sync_channel::<Job>(JOB_QUEUE_DEPTH);

    let ctx = Arc::new(ServerContext {
        config,
        session_token,
        status: Arc::clone(&status),
        jobs: JobQueue { tx },
        shutdown: Arc::clone(&shutdown),
        connections: Mutex::new(FnvHashMap::default()),
        next_connection_id: AtomicU64::new(0),
        live_sessions: AtomicUsize::new(0),
    });

    let worker_status = Arc::clone(&status);
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_thread = std::thread::Builder::new()
        .name("arnis-stream-worker".to_string())
        .spawn(move || generation_worker(rx, worker_status, worker_shutdown))
        .map_err(|e| format!("Could not start the stream generation worker: {e}"))?;

    let accept_ctx = Arc::clone(&ctx);
    let accept_thread = std::thread::Builder::new()
        .name("arnis-stream-accept".to_string())
        .spawn(move || accept_loop(listener, accept_ctx))
        .map_err(|e| format!("Could not start the stream accept loop: {e}"))?;

    status.set_activity("Waiting for a client");
    status.log(format!(
        "Stream mode listening on 127.0.0.1:{port} (tile {} + margin {}).",
        config.tile_size, config.margin
    ));

    Ok(StreamHandle {
        port,
        ctx,
        accept_thread: Some(accept_thread),
        worker_thread: Some(worker_thread),
        discovery_path,
    })
}

/// The one and only generation thread.
///
/// GENERATION IS SERIALISED ON PURPOSE, and a second worker would make the server wrong rather
/// than faster. `WORLD_MIN_Y`/`WORLD_MAX_Y` (`world_editor/common.rs`), `TERRAIN_FLOOR_Y` and
/// `BASE_CHUNK_Y` are process-global atomics that the pipeline retunes for the area it is
/// currently working on. Two jobs running concurrently would read each other's values halfway
/// through and produce terrain at the wrong height, with no error anywhere. The parallelism is
/// inside a job instead: one tile already fans out across ~90% of the machine's cores via rayon.
///
/// The same globals are also written by an ordinary generate-a-world-to-disk run, which this
/// worker knows nothing about, so every job additionally takes [`GenerationSlot`] — the same
/// process-wide slot the GUI's Generate button takes. A job that cannot get it is answered
/// `busy` instead of quietly corrupting the world someone is writing.
fn generation_worker(rx: Receiver<Job>, status: Arc<StreamStatus>, shutdown: Arc<AtomicBool>) {
    loop {
        // On shutdown, throw away whatever is queued instead of running it: nobody is waiting
        // for it any more, and each dropped job drops the response channel it captured, which is
        // what lets the connection's writer thread exit.
        if shutdown.load(Ordering::Relaxed) {
            while rx.try_recv().is_ok() {}
            break;
        }
        let job = match rx.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let Job {
            label,
            run,
            on_panic,
            on_busy,
        } = job;

        // Claim the process before the job touches a single global. A disk generation holding
        // the slot is the common case worth being careful about: it writes the same world
        // bounds, terrain floor and filler base, and a tile job retuning them mid-run would
        // corrupt the world being written, silently and with no error anywhere.
        let Some(_generation_slot) = GenerationSlot::acquire() else {
            status.log(format!(
                "Job '{label}' refused: a world generation is already running."
            ));
            let _ = std::panic::catch_unwind(AssertUnwindSafe(on_busy));
            continue;
        };

        // The pipeline reports its stages to whatever GUI window is listening. Those emits
        // belong to a generation the user started, not to a tile nobody asked for, so the whole
        // job runs muted — bar, status line, ETA and the `Error!` that would re-enable the
        // Generate button under a running generation.
        let _mute = ProgressMute::new();

        status.set_activity(format!("Generating {label}"));

        // One malformed request must not kill the server. The pipeline panics in several places
        // and `catch_unwind` is the only thing between that and a dead worker thread. The
        // default panic hook still prints the backtrace, which is what we want in the log.
        match std::panic::catch_unwind(AssertUnwindSafe(run)) {
            Ok(()) => {}
            Err(payload) => {
                let detail = panic_message(payload.as_ref());
                status.log(format!("Job '{label}' failed: {detail}"));
                // Reporting the failure is itself fallible (the client may have vanished), and a
                // panic in the reporter would take down the worker the outer catch just saved.
                let _ = std::panic::catch_unwind(AssertUnwindSafe(move || on_panic(detail)));
            }
        }
        status.set_activity("Idle");
    }
}

/// Best-effort text of a caught panic. `panic!` payloads are `&'static str` or `String`;
/// anything else came from a custom `panic_any` and has no printable form.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "internal error (panic with an unprintable payload)".to_string()
    }
}

/// Accept connections until [`StreamHandle::stop`] sets the shutdown flag.
///
/// Connection threads are collected so shutdown can join them; each finishes on its own once
/// [`ServerContext::shutdown_connections`] half-closes its socket and its reader sees the flag.
/// At most [`MAX_CONNECTIONS`] run at a time; anything beyond that is told so and closed.
fn accept_loop(listener: TcpListener, ctx: Arc<ServerContext>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();

    while !ctx.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, addr)) => {
                // The listener is non-blocking, and on the BSDs (macOS included) an accepted
                // socket inherits that. Sessions want blocking reads with a timeout, so set it
                // explicitly rather than relying on platform behaviour.
                if let Err(e) = stream.set_nonblocking(false) {
                    ctx.status
                        .log(format!("Rejecting connection from {addr}: {e}"));
                    continue;
                }
                // Reap first, so only genuinely live sessions count against the cap.
                connections.retain(|handle| !handle.is_finished());
                if connections.len() >= MAX_CONNECTIONS {
                    let live = connections.len();
                    ctx.status.log(format!(
                        "Refusing connection from {addr}: already serving {live} client(s)."
                    ));
                    reject_connection(&mut stream);
                    continue;
                }
                ctx.status.log(format!("Client connected from {addr}."));
                let conn_ctx = Arc::clone(&ctx);
                match std::thread::Builder::new()
                    .name("arnis-stream-session".to_string())
                    .spawn(move || session::serve_connection(stream, conn_ctx))
                {
                    Ok(handle) => connections.push(handle),
                    Err(e) => ctx
                        .status
                        .log(format!("Could not start a session thread: {e}")),
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                // Reap finished sessions here so a long-lived server does not accumulate
                // handles for every client that has ever connected.
                connections.retain(|handle| !handle.is_finished());
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => {
                ctx.status.log(format!("Accept failed: {e}"));
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
        }
    }

    for handle in connections {
        let _ = handle.join();
    }
}

/// Tell a client we are already full, then close its socket.
///
/// A `HelloError` rather than a bare disconnect: the client has not sent anything yet, so a
/// silent close is indistinguishable from Arnis having crashed, and the mod would retry forever.
/// Request id 0 because there is no request to correlate with — the connection ends here.
fn reject_connection(stream: &mut TcpStream) {
    let msg = protocol::ServerMessage::HelloError(protocol::HelloError {
        reason: format!(
            "Arnis is already serving {MAX_CONNECTIONS} stream connection(s). Stream mode \
             bridges one Minecraft instance at a time; close the other one and reconnect."
        ),
        code: "busy".to_string(),
    });
    // Written straight to the socket, which is unbuffered, so there is nothing to flush.
    if let Ok(frame) = protocol::encode_server(&msg, 0) {
        let _ = protocol::write_frame(stream, &frame);
    }
    // Half-close rather than `Both`: the frame just written still has to reach the client, and
    // the socket is dropped (and so fully closed) as soon as this returns anyway.
    let _ = stream.shutdown(Shutdown::Write);
}

/// The process-wide handle, so the GUI can start and stop stream mode without threading a
/// [`StreamHandle`] through Tauri's managed state (which would have to be `Send + Sync` and
/// would still need this mutex).
static RUNNING: OnceLock<Mutex<Option<StreamHandle>>> = OnceLock::new();

fn running_slot() -> &'static Mutex<Option<StreamHandle>> {
    RUNNING.get_or_init(|| Mutex::new(None))
}

/// Start stream mode and keep the handle process-globally. Returns the bound port.
///
/// Starting while it is already running is an error rather than a silent restart: the caller
/// asked for a server and there is one, but its port and session token are not the ones the
/// caller is about to be told about.
pub fn start_global(config: StreamConfig) -> Result<u16, String> {
    let mut slot = running_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(handle) = slot.as_ref() {
        return Err(format!(
            "Stream mode is already running on port {}.",
            handle.port()
        ));
    }
    // A detached stop may still be releasing the listener. Binding before it finishes would fail
    // with "port already in use" against our own dying server, so wait for it here -- by the time
    // a user has toggled stream mode back on this has almost always already completed.
    join_pending_stop();
    let handle = start(config)?;
    let port = handle.port();
    *slot = Some(handle);
    Ok(port)
}

/// A detached [`stop_if_running_detached`] still winding down, joined by the next start.
static STOPPING: OnceLock<Mutex<Option<std::thread::JoinHandle<()>>>> = OnceLock::new();

fn stopping_slot() -> &'static Mutex<Option<std::thread::JoinHandle<()>>> {
    STOPPING.get_or_init(|| Mutex::new(None))
}

/// Wait for a previous detached stop to finish, if one is still running.
fn join_pending_stop() {
    let pending = {
        let mut slot = stopping_slot().lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    if let Some(handle) = pending {
        let _ = handle.join();
    }
}

/// Stop the process-global server without blocking the caller.
///
/// [`StreamHandle::stop`] waits for the in-flight tile job to finish, which can take as long as an
/// OSM fetch plus a full generation. The GUI calls this from the stream window's `Destroyed`
/// handler, which runs on the Tauri event-loop thread, so waiting inline would freeze the entire
/// application for that whole time.
///
/// The handle is taken from the slot synchronously, so [`global_status`] reports "not running"
/// immediately and nothing can attach to the dying server; only the wait is detached. The next
/// [`start_global`] joins it before binding, so a stop/start cycle cannot race on the port.
pub fn stop_if_running_detached() {
    let handle = {
        let mut slot = running_slot().lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    let Some(handle) = handle else { return };

    join_pending_stop();
    let joiner = std::thread::spawn(move || handle.stop());
    *stopping_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(joiner);
}

/// Stop the process-global server if one is running, waiting for it to finish.
///
/// A no-op otherwise, so it is safe to call on application exit unconditionally. Use
/// [`stop_if_running_detached`] from any thread that must not block.
pub fn stop_if_running() {
    let handle = {
        let mut slot = running_slot().lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    if let Some(handle) = handle {
        handle.stop();
    }
    // Settle a detached stop too, so calling this on application exit really does leave nothing
    // running and no discovery file behind.
    join_pending_stop();
}

/// Status of the process-global server, or `None` when stream mode is not running.
pub fn global_status() -> Option<StreamStatusSnapshot> {
    let slot = running_slot().lock().unwrap_or_else(|e| e.into_inner());
    slot.as_ref().map(|handle| handle.status().snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::protocol::{
        decode_chunk, AnchorSpec, Frame, GenConfig, Hello, HelloError, HelloOk, SectionPayload,
        VerticalMapping,
    };
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::panic::AssertUnwindSafe;
    use std::time::Duration;

    /// Stream mode has four pieces of process-global state — the discovery file (one path per
    /// machine), the world-bounds atomics a tile job sets, the `GENERATION_ACTIVE` slot, and the
    /// `RUNNING` slot — so these tests run one at a time even under the default parallel test
    /// runner.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// A client that hangs must fail the test rather than wedge the whole suite.
    const CLIENT_TIMEOUT: Duration = Duration::from_secs(120);

    struct TestServer {
        port: u16,
        token: String,
        /// The live status block, so a test can assert on what the stream window would show.
        status: Arc<StreamStatus>,
    }

    /// Wait for `check` to hold, or fail the test. Used where the thing under test happens on
    /// another thread (a session tearing down, a worker picking a job up) and polling is the
    /// only honest way to observe it.
    fn wait_until(what: &str, check: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    /// A [`ServerContext`] with no listener and no worker behind it, for the pieces that can be
    /// exercised without a live server.
    fn test_context(shutdown: bool) -> (Arc<ServerContext>, Receiver<Job>) {
        let (tx, rx) = sync_channel::<Job>(JOB_QUEUE_DEPTH);
        let ctx = Arc::new(ServerContext {
            config: StreamConfig::default(),
            session_token: "0".repeat(32),
            status: Arc::new(StreamStatus::new(0)),
            jobs: JobQueue { tx },
            shutdown: Arc::new(AtomicBool::new(shutdown)),
            connections: Mutex::new(FnvHashMap::default()),
            next_connection_id: AtomicU64::new(0),
            live_sessions: AtomicUsize::new(0),
        });
        (ctx, rx)
    }

    /// Start a server on an ephemeral port, run `body` against it, then stop it and check that
    /// stopping cleaned up after itself.
    fn with_server(body: impl FnOnce(&TestServer)) {
        let _serialize = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A tile job retunes the world-bounds globals that the world-editor tests read, so take
        // their lock too and hand the defaults back afterwards.
        let _bounds = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::world_editor::set_world_bounds(
            crate::world_editor::DEFAULT_MIN_Y,
            crate::world_editor::DEFAULT_MAX_Y,
        );

        let handle = start(StreamConfig {
            port: 0,
            ..StreamConfig::default()
        })
        .expect("stream mode should start on an ephemeral port");
        let port = handle.port();
        assert_ne!(port, 0, "port 0 must be resolved to the real bound port");

        let discovery = discovery_path().expect("a platform configuration directory");
        let published: DiscoveryFile = serde_json::from_str(
            &std::fs::read_to_string(&discovery).expect("the discovery file should exist"),
        )
        .expect("the discovery file should be valid JSON");
        assert_eq!(published.port, port);
        assert_eq!(published.pid, std::process::id());
        assert_eq!(published.protocol_version, protocol::PROTOCOL_VERSION);
        assert_eq!(published.session_token.len(), 32);

        let server = TestServer {
            port,
            token: published.session_token,
            status: handle.status(),
        };
        // Stop the server even when the body fails, or the next test inherits a live one.
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| body(&server)));

        handle.stop();
        assert!(
            !discovery.exists(),
            "a clean stop must delete the discovery file"
        );
        crate::world_editor::set_world_bounds(
            crate::world_editor::DEFAULT_MIN_Y,
            crate::world_editor::DEFAULT_MAX_Y,
        );

        if let Err(payload) = outcome {
            std::panic::resume_unwind(payload);
        }
    }

    fn connect(port: u16) -> TcpStream {
        let socket =
            TcpStream::connect(("127.0.0.1", port)).expect("the stream server should accept");
        socket
            .set_read_timeout(Some(CLIENT_TIMEOUT))
            .expect("set a client read timeout");
        socket
    }

    fn send(socket: &mut TcpStream, msg_type: u8, request_id: u64, body: &impl serde::Serialize) {
        let payload = serde_json::to_vec(body).expect("serialise the test message");
        protocol::write_frame(
            socket,
            &Frame {
                msg_type,
                request_id,
                payload,
            },
        )
        .expect("write a frame");
        socket.flush().expect("flush the frame");
    }

    fn recv(socket: &mut TcpStream) -> Frame {
        protocol::read_frame(socket).expect("a reply frame")
    }

    /// The next frame that ends a request. `Progress` is informational and may appear any number
    /// of times before the real answer.
    fn recv_terminal(socket: &mut TcpStream) -> Frame {
        loop {
            let frame = recv(socket);
            if frame.msg_type != protocol::MSG_PROGRESS {
                return frame;
            }
        }
    }

    /// A `Hello` that passes every check, for tests that want to vary exactly one thing.
    fn hello(token: &str) -> Hello {
        Hello {
            protocol_version: protocol::PROTOCOL_VERSION,
            client_name: "arnis-stream-tests".to_string(),
            client_version: "0.0.0".to_string(),
            session_token: token.to_string(),
            config: GenConfig {
                scale: 1.0,
                fillground: false,
                interior: false,
                use3d: false,
                overture: false,
                canopy_height: false,
                terrain_only: false,
                flat_ground: false,
                local_osm_file: None,
            },
            // -64/384/0 is the vanilla 1.18+ dimension with sea level at Y 0, so block Y reads
            // as metres above sea level.
            vertical: VerticalMapping {
                min_y: -64,
                height: 384,
                sea_level_y: 0,
                vertical_scale: 1.0,
            },
            anchors: Vec::new(),
        }
    }

    fn as_json<T: for<'de> serde::Deserialize<'de>>(frame: &Frame) -> T {
        serde_json::from_slice(&frame.payload).expect("a JSON reply body")
    }

    #[test]
    fn a_client_handshakes_pings_and_the_server_stops_cleanly() {
        with_server(|server| {
            let mut socket = connect(server.port);

            send(&mut socket, protocol::MSG_HELLO, 1, &hello(&server.token));
            let frame = recv(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_HELLO_OK);
            assert_eq!(frame.request_id, 1, "the request id must be echoed");
            let ok: HelloOk = as_json(&frame);
            assert_eq!(ok.protocol_version, protocol::PROTOCOL_VERSION);
            assert_eq!(ok.max_in_flight, MAX_IN_FLIGHT);
            assert_eq!(ok.tile_size % 16, 0, "a tile must be whole chunks");

            send(&mut socket, protocol::MSG_PING, 7, &serde_json::json!({}));
            let frame = recv(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_PONG);
            assert_eq!(frame.request_id, 7);
        });
    }

    #[test]
    fn a_wrong_session_token_is_rejected() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut bad = hello(&server.token);
            bad.session_token = "0".repeat(32);

            send(&mut socket, protocol::MSG_HELLO, 1, &bad);
            let frame = recv(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_HELLO_ERROR);
            let err: HelloError = as_json(&frame);
            assert_eq!(err.code, "bad_token");
            assert!(!err.reason.is_empty(), "the reason is shown to a user");
        });
    }

    /// The important half of this test is the second assertion. `set_world_bounds` asserts
    /// 16-alignment in release builds too, so a handshake that reached it with an unaligned
    /// floor would abort the whole test process instead of failing this case.
    #[test]
    fn an_unaligned_world_floor_is_rejected_before_it_reaches_the_world_bounds() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut bad = hello(&server.token);
            bad.vertical.min_y = -60;

            send(&mut socket, protocol::MSG_HELLO, 1, &bad);
            let frame = recv(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_HELLO_ERROR);
            let err: HelloError = as_json(&frame);
            assert_eq!(err.code, "invalid_vertical_mapping");

            assert_eq!(
                crate::world_editor::min_y(),
                crate::world_editor::DEFAULT_MIN_Y,
                "a rejected mapping must not have touched the world bounds"
            );
        });
    }

    #[test]
    fn overlapping_anchors_are_rejected() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut bad = hello(&server.token);
            // Two patches a kilometre apart in both worlds, each claiming a 5 km radius.
            bad.anchors = vec![
                AnchorSpec {
                    id: 0,
                    lat: 54.630,
                    lon: 9.930,
                    mc_x: 0,
                    mc_z: 0,
                    radius_m: 5_000.0,
                },
                AnchorSpec {
                    id: 1,
                    lat: 54.635,
                    lon: 9.935,
                    mc_x: 500,
                    mc_z: 0,
                    radius_m: 5_000.0,
                },
            ];

            send(&mut socket, protocol::MSG_HELLO, 1, &bad);
            let frame = recv(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_HELLO_ERROR);
            let err: HelloError = as_json(&frame);
            assert_eq!(err.code, "invalid_anchors");
        });
    }

    /// A chunk outside every registered patch is an error, not an empty chunk: the server has no
    /// defined mapping there. Fully offline — the request is rejected before any tile job.
    #[test]
    fn a_chunk_outside_every_patch_is_reported_rather_than_generated() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut with_anchor = hello(&server.token);
            with_anchor.anchors = vec![AnchorSpec {
                id: 1,
                lat: 54.6313,
                lon: 9.9308,
                mc_x: 0,
                mc_z: 0,
                radius_m: 1_000.0,
            }];
            send(&mut socket, protocol::MSG_HELLO, 1, &with_anchor);
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);

            // 10 000 blocks out, far beyond the 1 km patch.
            send(
                &mut socket,
                protocol::MSG_REQUEST_CHUNK,
                2,
                &serde_json::json!({ "chunkX": 625, "chunkZ": 625 }),
            );
            let frame = recv_terminal(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_ERROR);
            let err: protocol::ErrorMessage = as_json(&frame);
            assert_eq!(err.code, "out_of_patch");

            // A named anchor that does not exist is a different, more specific failure.
            send(
                &mut socket,
                protocol::MSG_REQUEST_CHUNK,
                3,
                &serde_json::json!({ "chunkX": 0, "chunkZ": 0, "anchorId": 99 }),
            );
            let frame = recv_terminal(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_ERROR);
            let err: protocol::ErrorMessage = as_json(&frame);
            assert_eq!(err.code, "unknown_anchor");
        });
    }

    /// `Locate` answers coordinates and refuses place names, both without any network access.
    #[test]
    fn locate_resolves_coordinates_and_declines_place_names() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut with_anchor = hello(&server.token);
            with_anchor.anchors = vec![AnchorSpec {
                id: 1,
                lat: 54.6313,
                lon: 9.9308,
                mc_x: 0,
                mc_z: 0,
                radius_m: 10_000.0,
            }];
            send(&mut socket, protocol::MSG_HELLO, 1, &with_anchor);
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);

            send(
                &mut socket,
                protocol::MSG_LOCATE,
                2,
                &serde_json::json!({ "query": "54.6313, 9.9308" }),
            );
            let frame = recv_terminal(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_JSON_REPLY);
            let reply: protocol::JsonReply = as_json(&frame);
            match reply {
                protocol::JsonReply::Locate(locate) => {
                    assert!(locate.found);
                    assert_eq!(locate.anchor_id, Some(1));
                    // The anchor's own position projects exactly onto its pinned block.
                    assert_eq!(locate.mc_x, Some(0));
                    assert_eq!(locate.mc_z, Some(0));
                }
                other => panic!("expected a locate reply, got {other:?}"),
            }

            send(
                &mut socket,
                protocol::MSG_LOCATE,
                3,
                &serde_json::json!({ "query": "Arnis, Germany" }),
            );
            let frame = recv_terminal(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_ERROR);
            let err: protocol::ErrorMessage = as_json(&frame);
            assert_eq!(err.code, "geocoding_unavailable");
        });
    }

    /// End-to-end: handshake, ask for a chunk over the offline `.osm` fixture, and decode the
    /// `ChunkData` that comes back.
    ///
    /// Fully offline, so it runs in CI. `localOsmFile` replaces the Overpass query and
    /// `flatGround` skips the elevation, land-cover and canopy fetches, which are the only other
    /// network traffic a tile job produces.
    #[test]
    fn a_chunk_request_returns_decodable_chunk_data() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut request = hello(&server.token);
            request.config.local_osm_file =
                Some(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.osm").to_string());
            // Flat ground keeps the job off the network entirely.
            request.config.flat_ground = true;
            // The fixture's building sits between 54.6308..54.6313 N and 9.9308..9.9314 E. The
            // anchor pins its north-west corner to block 0,0, so the building runs east and
            // south from there and chunk 0,0 lands inside it.
            request.anchors = vec![AnchorSpec {
                id: 1,
                lat: 54.6313,
                lon: 9.9308,
                mc_x: 0,
                mc_z: 0,
                radius_m: 5_000.0,
            }];

            send(&mut socket, protocol::MSG_HELLO, 1, &request);
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);

            send(
                &mut socket,
                protocol::MSG_REQUEST_CHUNK,
                42,
                &serde_json::json!({ "chunkX": 0, "chunkZ": 0, "anchorId": 1 }),
            );
            let frame = recv_terminal(&mut socket);
            if frame.msg_type == protocol::MSG_ERROR {
                let err: protocol::ErrorMessage = as_json(&frame);
                panic!("chunk request failed: {} - {}", err.code, err.message);
            }
            assert_eq!(frame.msg_type, protocol::MSG_CHUNK_DATA);
            assert_eq!(frame.request_id, 42);

            let chunk = decode_chunk(&frame.payload).expect("ChunkData should decode");
            assert_eq!(chunk.chunk_x, 0);
            assert_eq!(chunk.chunk_z, 0);
            assert!(
                chunk
                    .sections
                    .iter()
                    .any(|section| !matches!(section, SectionPayload::Empty)),
                "the fixture's building and street must produce at least one non-air section"
            );
        });
    }

    #[test]
    fn a_bind_failure_reads_as_a_sentence_not_an_io_error() {
        let in_use = describe_bind_error(
            41234,
            &std::io::Error::new(ErrorKind::AddrInUse, "address in use"),
        );
        assert_eq!(
            in_use,
            "Port 41234 is already in use. Try a different port."
        );

        let denied = describe_bind_error(
            80,
            &std::io::Error::new(ErrorKind::PermissionDenied, "denied"),
        );
        assert!(denied.contains("above 1024"), "{denied}");

        // Anything unexpected still has to be a sentence rather than a Debug dump.
        let other = describe_bind_error(1234, &std::io::Error::other("something else"));
        assert!(
            other.starts_with("Could not start the stream server"),
            "{other}"
        );
        assert!(!other.contains("Os {"), "{other}");
    }

    #[test]
    fn a_session_token_is_thirty_two_hex_characters() {
        let token = generate_session_token();
        assert_eq!(token.len(), 32);
        assert!(token
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
        assert_ne!(token, generate_session_token(), "tokens must not repeat");
    }

    /// Moved here with the guard itself: the slot is no longer the GUI's private business, it
    /// is the one thing that keeps a stream tile job and a disk generation off each other's
    /// process globals. Takes `TEST_LOCK` because the stream tests below generate tiles, which
    /// now take the same slot.
    #[test]
    fn a_second_generation_is_refused_until_the_first_finishes() {
        let _serialize = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let first = GenerationSlot::acquire().expect("the first generation must get the slot");
        assert!(
            GenerationSlot::acquire().is_none(),
            "a second generation must be refused while the first holds the slot"
        );
        drop(first);
        assert!(
            GenerationSlot::acquire().is_some(),
            "the slot must be free again once the first generation finishes"
        );
    }

    /// A job whose receiver is gone must be REPORTED, not counted as enqueued: the failed send
    /// destroys the job, so nothing else will ever answer the request it was carrying.
    #[test]
    fn submitting_to_a_closed_queue_is_an_error_rather_than_a_silent_swallow() {
        let (tx, rx) = sync_channel::<Job>(JOB_QUEUE_DEPTH);
        let queue = JobQueue { tx };
        drop(rx);

        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let outcome = queue.submit(Job {
            label: "closed".to_string(),
            run: Box::new(move || flag.store(true, Ordering::Relaxed)),
            on_panic: Box::new(|_| {}),
            on_busy: Box::new(|| {}),
        });

        assert_eq!(outcome, Err(SubmitError::Closed));
        assert!(!ran.load(Ordering::Relaxed), "the job cannot have run");
        // The waiter is answered as a failure, not asked to retry something that can never work.
        assert_eq!(SubmitError::Closed.code(), "generation_failed");
        assert_eq!(SubmitError::Full.code(), "busy");
    }

    /// A full queue is still ordinary back-pressure.
    #[test]
    fn a_full_queue_is_reported_as_busy() {
        let (tx, _rx) = sync_channel::<Job>(1);
        let queue = JobQueue { tx };
        let job = || Job {
            label: "full".to_string(),
            run: Box::new(|| {}),
            on_panic: Box::new(|_| {}),
            on_busy: Box::new(|| {}),
        };
        assert_eq!(queue.submit(job()), Ok(()));
        assert_eq!(queue.submit(job()), Err(SubmitError::Full));
    }

    /// The shutdown race: a socket accepted while `stop` is draining the connection map must be
    /// closed by whoever loses the race, or its reader parks in an untimed `read` that nothing
    /// will ever wake and the accept loop's join never returns.
    #[test]
    fn a_connection_registered_after_shutdown_is_closed_rather_than_parked() {
        let (ctx, _rx) = test_context(true);
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a scratch listener");
        let addr = listener.local_addr().expect("read back the scratch port");
        let mut client = TcpStream::connect(addr).expect("connect to the scratch listener");
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("set a client read timeout");
        let (accepted, _) = listener.accept().expect("accept the scratch connection");

        ctx.register_connection(&accepted);

        assert!(
            ctx.connections
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "a socket registered after shutdown must not enter the map that was already drained"
        );
        let mut buf = [0u8; 1];
        assert_eq!(
            client
                .read(&mut buf)
                .expect("the socket must have been closed"),
            0,
            "the socket must be half-closed, so its reader cannot park forever"
        );
    }

    /// The same race from the outside: a client that connects and then says nothing must not be
    /// able to wedge `stop()`.
    #[test]
    fn stop_returns_while_a_silent_client_is_connected() {
        let _serialize = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let handle = start(StreamConfig {
            port: 0,
            ..StreamConfig::default()
        })
        .expect("stream mode should start on an ephemeral port");
        let port = handle.port();
        let status = handle.status();

        // Connect and never write a byte.
        let socket = TcpStream::connect(("127.0.0.1", port)).expect("the server should accept");
        wait_until("the server to log the connection", || {
            status
                .snapshot()
                .log
                .iter()
                .any(|line| line.contains("Client connected"))
        });

        let stopped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stopped);
        let stopper = std::thread::spawn(move || {
            handle.stop();
            flag.store(true, Ordering::SeqCst);
        });
        wait_until("stop() to return with a silent client connected", || {
            stopped.load(Ordering::SeqCst)
        });
        let _ = stopper.join();
        drop(socket);
    }

    /// The one-client rule used to be a comment. Past the cap a client is told why and closed,
    /// rather than getting a session thread of its own.
    #[test]
    fn beyond_the_connection_cap_a_client_is_told_rather_than_silently_served() {
        with_server(|server| {
            let mut held = Vec::new();
            for _ in 0..MAX_CONNECTIONS {
                let mut socket = connect(server.port);
                send(&mut socket, protocol::MSG_HELLO, 1, &hello(&server.token));
                assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);
                held.push(socket);
            }

            let mut extra = connect(server.port);
            let frame = recv(&mut extra);
            assert_eq!(frame.msg_type, protocol::MSG_HELLO_ERROR);
            let err: HelloError = as_json(&frame);
            assert_eq!(err.code, "busy");
            assert!(!err.reason.is_empty(), "the reason is shown to a user");

            // The clients already connected are untouched.
            let first = held.first_mut().expect("a held connection");
            send(first, protocol::MSG_PING, 9, &serde_json::json!({}));
            assert_eq!(recv(first).msg_type, protocol::MSG_PONG);
        });
    }

    /// Any local process can open the port and close it again. That must not blank out the
    /// status of the client that is actually connected.
    #[test]
    fn a_probe_connection_ending_leaves_the_real_clients_status_alone() {
        with_server(|server| {
            let mut socket = connect(server.port);
            send(&mut socket, protocol::MSG_HELLO, 1, &hello(&server.token));
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);
            assert!(server.status.client_connected.load(Ordering::Relaxed));

            // A second socket that never handshakes: a port scan, or a mod that crashed on start.
            drop(connect(server.port));
            wait_until("the probe connection to be torn down", || {
                server
                    .status
                    .snapshot()
                    .log
                    .iter()
                    .any(|line| line.contains("disconnected"))
            });

            let snapshot = server.status.snapshot();
            assert!(
                snapshot.client_connected,
                "the handshaked client is still connected"
            );
            assert_eq!(
                snapshot.client_name.as_deref(),
                Some("arnis-stream-tests"),
                "the probe must not have cleared the real client's identity"
            );
            send(&mut socket, protocol::MSG_PING, 3, &serde_json::json!({}));
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_PONG);
        });
    }

    /// Every job runs muted, so a tile cannot drive the MAIN window's progress bar, and every
    /// job is refused rather than run while a world generation owns the process globals.
    #[test]
    fn a_job_runs_muted_and_is_refused_while_a_generation_holds_the_slot() {
        let _serialize = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = sync_channel::<Job>(JOB_QUEUE_DEPTH);
        let status = Arc::new(StreamStatus::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn({
            let status = Arc::clone(&status);
            let shutdown = Arc::clone(&shutdown);
            move || generation_worker(rx, status, shutdown)
        });

        let (observed, outcomes) = std::sync::mpsc::channel::<(&'static str, u32)>();
        let queue = JobQueue { tx };

        let run_tx = observed.clone();
        let busy_tx = observed.clone();
        queue
            .submit(Job {
                label: "muted".to_string(),
                run: Box::new(move || {
                    let _ = run_tx.send(("run", progress_mutes()));
                }),
                on_panic: Box::new(|_| {}),
                on_busy: Box::new(move || {
                    let _ = busy_tx.send(("busy", 0));
                }),
            })
            .expect("the queue is empty");
        assert_eq!(
            outcomes
                .recv_timeout(Duration::from_secs(30))
                .expect("the worker should run the job"),
            ("run", 1),
            "the pipeline's progress emits must be muted for the whole job"
        );
        wait_until("the mute to be released", || progress_mutes() == 0);

        // Now with a world generation holding the slot.
        let generation = GenerationSlot::acquire().expect("no generation is running");
        let run_tx = observed.clone();
        let busy_tx = observed.clone();
        queue
            .submit(Job {
                label: "refused".to_string(),
                run: Box::new(move || {
                    let _ = run_tx.send(("run", progress_mutes()));
                }),
                on_panic: Box::new(|_| {}),
                on_busy: Box::new(move || {
                    let _ = busy_tx.send(("busy", 0));
                }),
            })
            .expect("the queue is empty");
        assert_eq!(
            outcomes
                .recv_timeout(Duration::from_secs(30))
                .expect("the worker should answer the job")
                .0,
            "busy",
            "a job must not generate while a world generation owns the process globals"
        );
        drop(generation);

        shutdown.store(true, Ordering::SeqCst);
        let _ = worker.join();
    }

    /// A coordinate a client can send but the server cannot represent is a `bad_request`, not a
    /// multiply that overflows on the connection thread and takes the session down with it.
    #[test]
    fn an_out_of_range_coordinate_is_a_bad_request_not_a_panic() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut with_anchor = hello(&server.token);
            with_anchor.anchors = vec![AnchorSpec {
                id: 1,
                lat: 54.6313,
                lon: 9.9308,
                mc_x: 0,
                mc_z: 0,
                radius_m: 1_000.0,
            }];
            send(&mut socket, protocol::MSG_HELLO, 1, &with_anchor);
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);

            // 2^27 chunks: the first thing the handler used to do was multiply this by 16.
            for (id, body) in [
                (2, serde_json::json!({ "chunkX": 134_217_728, "chunkZ": 0 })),
                (3, serde_json::json!({ "chunkX": i32::MIN, "chunkZ": 0 })),
            ] {
                send(&mut socket, protocol::MSG_REQUEST_CHUNK, id, &body);
                let frame = recv_terminal(&mut socket);
                assert_eq!(frame.msg_type, protocol::MSG_ERROR, "request {id}");
                let err: protocol::ErrorMessage = as_json(&frame);
                assert_eq!(err.code, "bad_request", "request {id}");
            }

            send(
                &mut socket,
                protocol::MSG_REQUEST_COLUMN,
                4,
                &serde_json::json!({ "x": i32::MIN, "z": 0 }),
            );
            let frame = recv_terminal(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_ERROR);
            let err: protocol::ErrorMessage = as_json(&frame);
            assert_eq!(err.code, "bad_request");

            send(
                &mut socket,
                protocol::MSG_PREFETCH,
                5,
                &serde_json::json!({ "chunkX": i32::MAX, "chunkZ": 0 }),
            );
            let frame = recv_terminal(&mut socket);
            assert_eq!(frame.msg_type, protocol::MSG_ERROR);
            let err: protocol::ErrorMessage = as_json(&frame);
            assert_eq!(err.code, "bad_request");

            // And the connection is still usable, which is the other half of "not a panic".
            send(&mut socket, protocol::MSG_PING, 6, &serde_json::json!({}));
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_PONG);
        });
    }

    /// The handshake validates the client's vertical mapping and STORES it. It must not write it
    /// into the process-global world bounds: that happens on the connection's own thread, where
    /// it can retune the dimension under a tile job or a disk generation already in progress.
    #[test]
    fn a_handshake_does_not_touch_the_process_global_world_bounds() {
        with_server(|server| {
            let mut socket = connect(server.port);
            let mut request = hello(&server.token);
            // A valid mapping that differs from the defaults, so the assertion has teeth.
            request.vertical = VerticalMapping {
                min_y: -128,
                height: 512,
                sea_level_y: 0,
                vertical_scale: 1.0,
            };
            send(&mut socket, protocol::MSG_HELLO, 1, &request);
            assert_eq!(recv(&mut socket).msg_type, protocol::MSG_HELLO_OK);

            assert_eq!(
                crate::world_editor::min_y(),
                crate::world_editor::DEFAULT_MIN_Y,
                "the handshake must leave the world bounds to the generation worker"
            );
        });
    }

    #[test]
    fn the_log_ring_buffer_is_bounded() {
        let status = StreamStatus::new(1234);
        for i in 0..(LOG_CAPACITY + 50) {
            status.log(format!("line {i}"));
        }
        let snapshot = status.snapshot();
        assert_eq!(snapshot.log.len(), LOG_CAPACITY);
        // The oldest lines are the ones dropped.
        assert_eq!(snapshot.log[0], "line 50");
        assert_eq!(snapshot.port, 1234);
    }
}
