//! One stream-mode connection: the handshake, the dispatch loop, and the reply writer.
//!
//! Three threads touch a connection:
//!
//! * the **reader** (this thread) parses frames and answers everything that cannot block —
//!   `Ping`, `Cancel`, `AddAnchor`, `Locate`, and chunk requests that hit the tile cache;
//! * the single **generation worker** (owned by [`crate::stream`]) runs everything that fetches
//!   or generates, one job at a time;
//! * the **writer** drains a response channel onto the socket, so a client that stops reading
//!   stalls only itself and never the worker.
//!
//! Everything a client fixes for the session — its generation settings, its vertical mapping,
//! its anchors — is settled by `Hello` and never renegotiated. See `docs/stream-protocol.md`
//! for the normative message catalogue.

use std::io::{BufWriter, ErrorKind, Write};
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use clap::Parser;
use fnv::FnvHashMap;

use crate::args::{Args, GenerationMode};
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::AbsoluteVerticalMapping;
use crate::projection::{Projection, EARTH_RADIUS};
use crate::stream::projection::{Anchor, AnchorSet, DEFAULT_ANCHOR_RADIUS_M};
use crate::stream::protocol::{
    self, AddAnchor, AnchorReply, Cancel, ChunkPayload, ClientMessage, ColumnReply,
    ElevationRangeReply, ErrorMessage, Frame, GenConfig, Hello, HelloError, HelloOk, JsonReply,
    Locate, LocateReply, Prefetch, Progress, QueryElevationRange, RequestChunk, RequestColumn,
    SectionPayload, ServerMessage, VerticalMapping,
};
use crate::stream::tiles::{self, AnchorDigest, GeneratedTile, TileCache, TileJobConfig, TileKey};
use crate::stream::{AnchorSummary, Job, ServerContext, StreamStatus, SubmitError, MAX_IN_FLIGHT};

/// Hard ceiling on a patch radius, from `docs/stream-protocol.md`. Beyond this the anchor's own
/// transverse Mercator starts to visibly distort geometry at the rim.
const MAX_ANCHOR_RADIUS_M: f64 = 500_000.0;

/// Blocks of headroom left above the highest terrain in a `QueryElevationRange` recommendation,
/// for buildings, trees and the tallest structures Arnis places.
const RECOMMENDED_HEADROOM: i32 = 128;

/// Blocks left below the lowest terrain, so the bedrock plane and the filled column under the
/// surface have somewhere to go. Matches `world_editor::common::TERRAIN_FLOOR_DEPTH`.
const RECOMMENDED_FLOOR_MARGIN: i32 = 64;

/// Highest legal `minY + height`: block Y is packed into 12 bits, so 2031 is the top block.
const ENGINE_TOP: i32 = 2032;

/// Lowest legal `minY`, the mirror of [`ENGINE_TOP`].
const ENGINE_FLOOR: i32 = -2032;

/// Tallest legal world: the section index is a signed byte and one section is reserved at each
/// end for lighting, leaving 254 sections.
const MAX_WORLD_HEIGHT: i32 = 4064;

/// Chebyshev radius, in chunks, a `Prefetch` warms when the client does not say.
const DEFAULT_PREFETCH_RADIUS: u32 = 4;

/// Cap on a `Prefetch` radius, so one hint cannot enqueue an unbounded number of tiles.
const MAX_PREFETCH_RADIUS: u32 = 32;

/// Biome reported for a chunk that was never generated. Such a chunk is entirely air, so nothing
/// in it is coloured by a biome; the field still has to carry something valid.
const DEFAULT_BIOME: &str = "minecraft:plains";

/// Farthest block coordinate stream mode will accept from a client, on either axis.
///
/// Minecraft's own world border tops out at 29 999 984 blocks, so this rejects nothing a real
/// client can reach. What it does reject is a coordinate whose derived values would not fit in
/// an `i32`: `chunk * 16`, a tile rect's corners and that rect grown by the margin are all
/// computed in `i32`, and `overflow-checks` is on in release too, so an unvalidated coordinate
/// panics on the connection thread instead of returning an error.
pub(crate) const MAX_BLOCK_COORD: i32 = 30_000_000;

/// [`MAX_BLOCK_COORD`] expressed in chunks.
pub(crate) const MAX_CHUNK_COORD: i32 = MAX_BLOCK_COORD / 16;

/// Whether a chunk coordinate pair is inside [`MAX_CHUNK_COORD`].
///
/// `unsigned_abs`, not `abs`: `i32::MIN.abs()` is itself an overflow.
fn chunk_coords_in_range(chunk_x: i32, chunk_z: i32) -> bool {
    let limit = MAX_CHUNK_COORD.unsigned_abs();
    chunk_x.unsigned_abs() <= limit && chunk_z.unsigned_abs() <= limit
}

/// Whether a block coordinate pair is inside [`MAX_BLOCK_COORD`].
fn block_coords_in_range(x: i32, z: i32) -> bool {
    let limit = MAX_BLOCK_COORD.unsigned_abs();
    x.unsigned_abs() <= limit && z.unsigned_abs() <= limit
}

// -------------------------------------------------------------------------------------------
// Replies
// -------------------------------------------------------------------------------------------

/// The write side of a connection.
///
/// Cloned into every generation job, so a job can answer its own request without ever touching
/// the socket. The channel is unbounded on purpose: a bounded one would make the generation
/// worker block on a client that has stopped reading, which is exactly the coupling the writer
/// thread exists to prevent.
#[derive(Clone)]
struct Replies {
    tx: Sender<Frame>,
    status: Arc<StreamStatus>,
}

impl Replies {
    /// Queue one already-built frame. A send failure means the writer thread is gone (the client
    /// disconnected), which is not worth reporting per message — the reader will notice too.
    fn send_frame(&self, frame: Frame) {
        let _ = self.tx.send(frame);
    }

    /// Queue one server message.
    fn send(&self, msg: &ServerMessage, request_id: u64) {
        match protocol::encode_server(msg, request_id) {
            Ok(frame) => self.send_frame(frame),
            // An encoding failure is an Arnis bug, not a client one, so the client still gets a
            // terminal message for its request rather than silence.
            Err(reason) => {
                self.status
                    .log(format!("Failed to encode a reply: {reason}"));
                self.raw_error(
                    request_id,
                    "generation_failed",
                    format!("internal error encoding the reply: {reason}"),
                );
            }
        }
    }

    /// Terminal failure for one request. Never closes the connection.
    fn error(&self, request_id: u64, code: &str, message: impl Into<String>) {
        self.raw_error(request_id, code, message.into());
    }

    /// The one encode that must not recurse back into [`Replies::send`], because it is what
    /// `send` falls back to when encoding fails.
    fn raw_error(&self, request_id: u64, code: &str, message: String) {
        if let Ok(frame) = protocol::encode_server(
            &ServerMessage::Error(ErrorMessage {
                code: code.to_string(),
                message,
            }),
            request_id,
        ) {
            self.send_frame(frame);
        }
    }

    /// Handshake failure. The caller must close the connection afterwards.
    fn hello_error(&self, request_id: u64, code: &str, reason: impl Into<String>) {
        let reason = reason.into();
        self.status
            .log(format!("Handshake rejected ({code}): {reason}"));
        self.send(
            &ServerMessage::HelloError(HelloError {
                reason,
                code: code.to_string(),
            }),
            request_id,
        );
    }

    /// Informational stage note, so a client shows "fetching elevation" instead of a frozen bar.
    fn progress(&self, request_id: u64, stage: &str, detail: impl Into<String>) {
        self.send(
            &ServerMessage::Progress(Progress {
                stage: stage.to_string(),
                detail: detail.into(),
            }),
            request_id,
        );
    }
}

/// Drain the response channel onto the socket until the connection ends.
///
/// A `BufWriter` flushed once per frame turns the four small writes of a frame header and its
/// payload into one syscall, without ever holding a finished reply back waiting for the next.
fn writer_loop(socket: TcpStream, rx: Receiver<Frame>) {
    let mut out = BufWriter::new(socket);
    for frame in rx {
        if protocol::write_frame(&mut out, &frame).is_err() {
            break;
        }
        if out.flush().is_err() {
            break;
        }
    }
}

// -------------------------------------------------------------------------------------------
// In-flight bookkeeping
// -------------------------------------------------------------------------------------------

/// The generation-bearing requests one client currently has outstanding.
///
/// Doubles as the cancellation table: `Cancel` finds a request here and sets its flag, which the
/// pipeline reads at stage boundaries.
///
/// Also keeps [`StreamStatus::requests_in_flight`] equal to the sum of every connection's map,
/// which is what the stream window's "busy" indicator reads. The counter is maintained here
/// rather than at the call sites so it cannot drift: every path that fills a slot goes through
/// `claim`, every path that empties one through `release` or [`Drop`].
struct InFlight {
    slots: Mutex<FnvHashMap<u64, Arc<AtomicBool>>>,
    status: Arc<StreamStatus>,
}

impl InFlight {
    fn new(status: Arc<StreamStatus>) -> Self {
        Self {
            slots: Mutex::new(FnvHashMap::default()),
            status,
        }
    }

    fn lock(&self) -> MutexGuard<'_, FnvHashMap<u64, Arc<AtomicBool>>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claim a slot, returning the request's cancellation flag, or `None` when the client is
    /// already at `maxInFlight` — which the caller reports as `busy`, a flow-control signal
    /// rather than a fault.
    fn claim(&self, request_id: u64) -> Option<Arc<AtomicBool>> {
        let mut slots = self.lock();
        if slots.len() >= MAX_IN_FLIGHT as usize {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        // A repeated request id replaces the old entry; a client is not allowed to reuse one
        // while it is outstanding, and losing the stale flag beats refusing service. It must not
        // be counted twice, though: the replaced slot will only ever be released once.
        if slots.insert(request_id, Arc::clone(&flag)).is_none() {
            self.status
                .requests_in_flight
                .fetch_add(1, Ordering::Relaxed);
        }
        Some(flag)
    }

    /// Free a slot once its request has had its terminal message.
    fn release(&self, request_id: u64) {
        if self.lock().remove(&request_id).is_some() {
            self.decrement(1);
        }
    }

    /// Take `n` off the global counter without ever wrapping below zero.
    fn decrement(&self, n: u64) {
        let _ = self.status.requests_in_flight.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |v| Some(v.saturating_sub(n)),
        );
    }

    /// Flag one request, or every outstanding request when `request_id` is `None`.
    /// Cancelling something unknown or already finished is silently ignored, per the protocol.
    fn cancel(&self, request_id: Option<u64>) {
        let slots = self.lock();
        match request_id {
            Some(id) => {
                if let Some(flag) = slots.get(&id) {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            None => {
                for flag in slots.values() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

/// A connection that ends with requests still outstanding — a client that vanished, or a
/// shutdown that dropped queued jobs so `complete`/`fail` never ran — must not strand the
/// counter above zero, or the GUI's "busy" indicator stays lit until stream mode is restarted.
impl Drop for InFlight {
    fn drop(&mut self) {
        let stranded = self.lock().len() as u64;
        if stranded > 0 {
            self.decrement(stranded);
        }
    }
}

// -------------------------------------------------------------------------------------------
// Tile coalescing
// -------------------------------------------------------------------------------------------

/// Somebody waiting on a tile that is being generated.
enum Waiter {
    /// A `RequestChunk` that will be answered with `ChunkData`.
    Chunk {
        request_id: u64,
        chunk_x: i32,
        chunk_z: i32,
        cancel: Arc<AtomicBool>,
    },
    /// A `RequestColumn` that will be answered with a `column` JSON reply.
    Column {
        request_id: u64,
        x: i32,
        z: i32,
        cancel: Arc<AtomicBool>,
    },
    /// A `Prefetch` hint. Nobody is blocked on it and it gets no reply either way.
    Prefetch,
}

impl Waiter {
    fn is_cancelled(&self) -> bool {
        match self {
            Waiter::Chunk { cancel, .. } | Waiter::Column { cancel, .. } => {
                cancel.load(Ordering::Relaxed)
            }
            // A hint is never "cancelled": there is no request behind it for `Cancel` to name,
            // and a blanket cancel of the client's real requests should not throw away warming
            // work that is already under way.
            Waiter::Prefetch => false,
        }
    }

    /// The request id to correlate `Progress` messages with, if there is one.
    fn request_id(&self) -> Option<u64> {
        match self {
            Waiter::Chunk { request_id, .. } | Waiter::Column { request_id, .. } => {
                Some(*request_id)
            }
            Waiter::Prefetch => None,
        }
    }
}

/// One tile with a generation job already queued, plus everyone waiting on it.
///
/// This is what makes concurrent requests for the same tile cost one generation instead of N:
/// the first request creates the entry and the job, and every later request for the same tile
/// appends itself here and is answered from the same result.
struct PendingTile {
    /// Set only when *every* waiter has been cancelled, so a tile that still has one interested
    /// client keeps generating.
    cancel: Arc<AtomicBool>,
    waiters: Vec<Waiter>,
}

impl PendingTile {
    /// Attach another waiter to a tile that is already queued.
    ///
    /// A live waiter REVIVES the entry. Without that the cancel flag is sticky: once every
    /// waiter on a tile has been cancelled, `handle_cancel` sets the flag, and every later
    /// request that coalesces here is answered `cancelled` at the job's first stage boundary —
    /// a request nobody cancelled, for a tile that is then never generated. The
    /// player-leaves-and-returns pattern hits this on every return.
    fn coalesce(&mut self, waiter: Waiter) {
        if !waiter.is_cancelled() {
            self.cancel.store(false, Ordering::Relaxed);
        }
        self.waiters.push(waiter);
    }
}

// -------------------------------------------------------------------------------------------
// Session configuration
// -------------------------------------------------------------------------------------------

/// The parts of a [`TileJobConfig`] that `Hello` fixes for good.
///
/// Only the anchor list can change mid-session (`AddAnchor`), and it feeds nothing but the cache
/// invalidation hash, so the config is rebuilt from this basis rather than mutated in place —
/// jobs already on the queue keep the `Arc` they were given and stay self-consistent.
struct JobBasis {
    args: Arc<Args>,
    vertical: AbsoluteVerticalMapping,
    world_min_y: i32,
    world_height: i32,
    local_osm_file: Option<String>,
    tile_size: i32,
    margin: i32,
}

impl JobBasis {
    fn build(&self, anchors: &AnchorSet) -> TileJobConfig {
        TileJobConfig {
            args: Arc::clone(&self.args),
            vertical: self.vertical,
            world_min_y: self.world_min_y,
            world_height: self.world_height,
            local_osm_file: self.local_osm_file.clone(),
            tile_size: self.tile_size,
            margin: self.margin,
            anchors: anchors
                .anchors()
                .iter()
                .map(|anchor| AnchorDigest::of(anchor.id, anchor))
                .collect(),
        }
    }
}

/// Build the `Args` the generation pipeline consumes from the wire `GenConfig`.
///
/// Starting from clap's defaults rather than a hand-written literal means every field the
/// pipeline reads has its documented default, and only what the handshake actually settles is
/// overwritten. `parse_from` with no arguments cannot fail: every Arnis argument is optional.
fn args_from(config: &GenConfig, vertical: &VerticalMapping) -> Args {
    let mut args = Args::parse_from(["arnis"]);
    args.scale = config.scale;
    args.fillground = config.fillground;
    args.interior = config.interior;
    args.use_3d = config.use3d;
    args.overture = config.overture;
    args.canopy_height = config.canopy_height;
    args.mode = if config.terrain_only {
        GenerationMode::TerrainOnly
    } else if config.flat_ground {
        // Flat ground: no elevation, land cover or canopy fetch. `Ground::new_flat` then needs no
        // network at all, so with a local OSM extract a tile generates fully offline.
        GenerationMode::GeoOnly
    } else {
        GenerationMode::GeoTerrain
    };
    args.ground_level = vertical.sea_level_y;
    // Never on: the debug PNG dumps run inside `Ground` construction and write to bare relative
    // filenames, which a resident server has no business doing.
    args.debug = false;
    args
}

// -------------------------------------------------------------------------------------------
// Session
// -------------------------------------------------------------------------------------------

/// The parts of a session a generation job needs, all cheap to clone into a closure.
#[derive(Clone)]
struct Shared {
    ctx: Arc<ServerContext>,
    replies: Replies,
    basis: Arc<JobBasis>,
    /// Swapped wholesale when the anchor set changes; jobs capture the `Arc` they started with.
    job_config: Arc<Mutex<Arc<TileJobConfig>>>,
    tiles: Arc<Mutex<TileCache>>,
    pending: Arc<Mutex<FnvHashMap<TileKey, PendingTile>>>,
    in_flight: Arc<InFlight>,
}

impl Shared {
    fn pending(&self) -> MutexGuard<'_, FnvHashMap<TileKey, PendingTile>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tiles(&self) -> MutexGuard<'_, TileCache> {
        self.tiles.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn job_config(&self) -> Arc<TileJobConfig> {
        Arc::clone(&*self.job_config.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Rebuild the job config for a changed anchor set and tell the cache about it. A different
    /// config hash drops every resident tile, which is what keeps a moved anchor from leaving
    /// stale geometry behind.
    fn refresh_job_config(&self, anchors: &AnchorSet) {
        let config = Arc::new(self.basis.build(anchors));
        let hash = tiles::config_hash(&config);
        self.tiles().set_config_hash(hash);
        *self.job_config.lock().unwrap_or_else(|e| e.into_inner()) = config;
    }

    /// Take everyone waiting on a tile, clearing the entry so the next request starts fresh.
    fn take_waiters(&self, key: TileKey) -> Vec<Waiter> {
        self.pending()
            .remove(&key)
            .map(|entry| entry.waiters)
            .unwrap_or_default()
    }

    /// Answer one waiter from a finished tile.
    fn complete(&self, waiter: Waiter, tile: &GeneratedTile) {
        match waiter {
            Waiter::Chunk {
                request_id,
                chunk_x,
                chunk_z,
                cancel,
            } => {
                if cancel.load(Ordering::Relaxed) {
                    self.replies
                        .error(request_id, "cancelled", "request cancelled");
                } else {
                    self.send_chunk(request_id, tile, chunk_x, chunk_z);
                }
                self.in_flight.release(request_id);
            }
            Waiter::Column {
                request_id,
                x,
                z,
                cancel,
            } => {
                if cancel.load(Ordering::Relaxed) {
                    self.replies
                        .error(request_id, "cancelled", "request cancelled");
                } else {
                    self.send_column(request_id, tile, x, z);
                }
                self.in_flight.release(request_id);
            }
            Waiter::Prefetch => {}
        }
    }

    /// Fail one waiter. A prefetch hint has nobody to tell.
    fn fail(&self, waiter: Waiter, code: &str, message: &str) {
        match waiter {
            Waiter::Chunk { request_id, .. } | Waiter::Column { request_id, .. } => {
                self.replies.error(request_id, code, message);
                self.in_flight.release(request_id);
            }
            Waiter::Prefetch => {}
        }
    }

    /// Send one chunk, encoding straight from the tile's own payload.
    ///
    /// A chunk the tile does not contain is all air, not an error and not filler terrain: the
    /// mod is placing these into an existing world, so inventing a grass plane the way the disk
    /// writer does for empty regions would bulldoze whatever is already there.
    fn send_chunk(&self, request_id: u64, tile: &GeneratedTile, chunk_x: i32, chunk_z: i32) {
        let air;
        let payload = match tile.chunks.get(&(chunk_x, chunk_z)) {
            Some(payload) => payload,
            None => {
                air = air_chunk(chunk_x, chunk_z, tile.clipped);
                &air
            }
        };
        match protocol::encode_chunk(payload) {
            Ok(body) => {
                self.replies.send_frame(Frame {
                    msg_type: protocol::MSG_CHUNK_DATA,
                    request_id,
                    payload: body,
                });
                self.ctx
                    .status
                    .chunks_served
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(reason) => self.replies.error(
                request_id,
                "generation_failed",
                format!("could not encode chunk {chunk_x},{chunk_z}: {reason}"),
            ),
        }
    }

    /// Answer a surface probe out of an already-generated tile.
    fn send_column(&self, request_id: u64, tile: &GeneratedTile, x: i32, z: i32) {
        let chunk = tile.chunks.get(&(x.div_euclid(16), z.div_euclid(16)));
        // An empty column reports the world floor. It is not "no answer": the client asked where
        // the ground is and the honest answer for a column with nothing in it is the bottom.
        let surface_y = chunk
            .and_then(|payload| surface_y_in(payload, x, z))
            .unwrap_or(self.basis.world_min_y);
        self.replies.send(
            &ServerMessage::JsonReply(JsonReply::Column(ColumnReply {
                x,
                z,
                surface_y,
                clipped: tile.clipped,
            })),
            request_id,
        );
    }

    /// Answer everyone waiting on `key` from a tile this job owns, then cache it.
    fn resolve_with_tile(&self, key: TileKey, tile: GeneratedTile) {
        for waiter in self.take_waiters(key) {
            self.complete(waiter, &tile);
        }
        self.tiles().insert(key, tile);
    }

    /// Answer everyone waiting on `key` from the cache, which is expected to hold it.
    fn resolve_from_cache(&self, key: TileKey) {
        let waiters = self.take_waiters(key);
        if waiters.is_empty() {
            return;
        }
        let mut cache = self.tiles();
        match cache.get(key) {
            Some(tile) => {
                for waiter in waiters {
                    self.complete(waiter, tile);
                }
            }
            None => {
                drop(cache);
                for waiter in waiters {
                    self.fail(
                        waiter,
                        "generation_failed",
                        "the tile was evicted before it could be served",
                    );
                }
            }
        }
    }

    /// Fail everyone waiting on `key`.
    fn reject_pending(&self, key: TileKey, code: &str, message: &str) {
        for waiter in self.take_waiters(key) {
            self.fail(waiter, code, message);
        }
    }
}

/// One connection that has completed its handshake.
struct Session {
    shared: Shared,
    /// Guarded because `AddAnchor` mutates it while a queued job may be reading it.
    anchors: Arc<Mutex<AnchorSet>>,
}

impl Session {
    fn anchors(&self) -> MutexGuard<'_, AnchorSet> {
        self.anchors.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn replies(&self) -> &Replies {
        &self.shared.replies
    }

    fn tile_size(&self) -> i32 {
        self.shared.basis.tile_size
    }

    /// Publish the anchor list to the status block the GUI polls.
    fn publish_anchors(&self) {
        let summaries = self
            .anchors()
            .anchors()
            .iter()
            .map(|a| AnchorSummary {
                id: a.id,
                lat: a.lat,
                lon: a.lon,
                mc_x: a.mc_x,
                mc_z: a.mc_z,
                radius_m: a.radius_m,
            })
            .collect();
        self.shared.ctx.status.set_anchors(summaries);
    }

    // --- anchor resolution ------------------------------------------------------------------

    /// Which patch a block column belongs to.
    ///
    /// An explicit `anchorId` that was never registered is `unknown_anchor`; a column outside
    /// the named patch (or outside every patch, when none was named) is `out_of_patch`. Both are
    /// expected at patch edges and neither is a server fault.
    fn resolve_anchor(
        &self,
        anchor_id: Option<u32>,
        x: i32,
        z: i32,
    ) -> Result<Anchor, (&'static str, String)> {
        let anchors = self.anchors();
        let anchor = match anchor_id {
            Some(id) => *anchors.get(id).ok_or_else(|| {
                (
                    "unknown_anchor",
                    format!("anchor id {id} was never registered in this session"),
                )
            })?,
            None => *anchors.find_containing_mc(x, z).ok_or_else(|| {
                (
                    "out_of_patch",
                    format!("block ({x}, {z}) lies outside every registered patch"),
                )
            })?,
        };
        if !anchor.contains_mc(x, z) {
            return Err((
                "out_of_patch",
                format!(
                    "block ({x}, {z}) lies outside the patch of anchor {} \
                     (centre {},{}, radius {:.0} blocks)",
                    anchor.id,
                    anchor.mc_x,
                    anchor.mc_z,
                    // Blocks, to match the rest of the sentence: `radius_m` is metres, and the
                    // two only coincide at scale 1.0.
                    anchor.radius_blocks()
                ),
            ));
        }
        Ok(anchor)
    }

    // --- tile jobs --------------------------------------------------------------------------

    /// Attach `waiter` to the job for `key`, starting that job if it does not exist yet.
    ///
    /// `hint` marks a prefetch: it may be dropped when the worker queue is full, whereas a real
    /// request that cannot be queued is answered `busy`.
    fn enqueue_tile(&self, key: TileKey, anchor: Anchor, waiter: Waiter, hint: bool) {
        {
            let mut pending = self.shared.pending();
            if let Some(entry) = pending.get_mut(&key) {
                entry.coalesce(waiter);
                return;
            }
            pending.insert(
                key,
                PendingTile {
                    cancel: Arc::new(AtomicBool::new(false)),
                    waiters: vec![waiter],
                },
            );
        }

        let job = self.build_tile_job(key, anchor);
        let queued = if hint {
            self.shared.ctx.jobs.submit_hint(job)
        } else {
            self.shared.ctx.jobs.submit(job)
        };
        if let Err(e) = queued {
            // A closed queue is not back-pressure: the send destroyed the job, so nothing else
            // will ever answer these waiters. Say so, and free their in-flight slots.
            if e == SubmitError::Closed {
                self.shared.ctx.status.log(format!(
                    "Could not queue tile {},{}: {}",
                    key.tx,
                    key.tz,
                    e.message()
                ));
            }
            self.shared.reject_pending(key, e.code(), e.message());
        }
    }

    /// Build the worker job that generates one tile and answers everyone waiting on it.
    fn build_tile_job(&self, key: TileKey, anchor: Anchor) -> Job {
        let label = format!("tile {},{} of anchor {}", key.tx, key.tz, key.anchor_id);
        let config = self.shared.job_config();

        let shared = self.shared.clone();
        let job_label = label.clone();
        let run = move || {
            // The cancel flag and the request id used for `Progress` both come from the pending
            // entry, so a tile several requests coalesced onto reports against the first of
            // them. `Progress` is informational and the protocol says as much.
            let (cancel, progress_id) = {
                let pending = shared.pending();
                match pending.get(&key) {
                    Some(entry) => (
                        Arc::clone(&entry.cancel),
                        entry.waiters.iter().find_map(Waiter::request_id),
                    ),
                    // Every waiter left before the job started; nothing to do.
                    None => return,
                }
            };

            // Another request may have finished this tile while this job sat in the queue.
            // Bound to a local on purpose: a guard created inside an `if` condition lives until
            // the end of the whole `if` statement, and `resolve_from_cache` takes the same
            // (non-reentrant) lock.
            let already_cached = shared.tiles().contains(&key);
            if already_cached {
                shared.ctx.status.cache_hits.fetch_add(1, Ordering::Relaxed);
                shared.resolve_from_cache(key);
                return;
            }

            let detail = format!("tile {},{}", key.tx, key.tz);
            let progress = |stage: &str| {
                if let Some(request_id) = progress_id {
                    shared.replies.progress(request_id, stage, detail.clone());
                }
                shared.ctx.status.set_activity(format!("{stage} {detail}"));
            };

            match tiles::generate_tile(key, &anchor, &config, &cancel, &progress) {
                Ok(tile) => {
                    shared
                        .ctx
                        .status
                        .tiles_generated
                        .fetch_add(1, Ordering::Relaxed);
                    shared.resolve_with_tile(key, tile);
                }
                Err(reason) if reason == "cancelled" => {
                    shared.reject_pending(key, "cancelled", "request cancelled");
                }
                Err(reason) => {
                    shared
                        .ctx
                        .status
                        .log(format!("{job_label} failed: {reason}"));
                    shared.reject_pending(key, "generation_failed", &reason);
                }
            }
        };

        // A panic anywhere in the pipeline still owes every waiter a terminal message, or the
        // client hangs on a request that will never be answered.
        let panic_shared = self.shared.clone();
        let on_panic = move |detail: String| {
            panic_shared.reject_pending(
                key,
                "generation_failed",
                &format!("generation panicked while building the tile: {detail}"),
            );
        };

        // Same obligation when the worker cannot claim the process because a world generation
        // is running: the waiters owe a terminal frame either way.
        let busy_shared = self.shared.clone();
        let on_busy = move || {
            busy_shared.reject_pending(
                key,
                "busy",
                "Arnis is generating a world right now; retry this request when it finishes",
            );
        };

        Job {
            label,
            run: Box::new(run),
            on_panic: Box::new(on_panic),
            on_busy: Box::new(on_busy),
        }
    }

    // --- handlers ---------------------------------------------------------------------------

    fn handle_request_chunk(&self, request_id: u64, req: RequestChunk) {
        // Before anything multiplies the coordinate by 16. See `MAX_CHUNK_COORD`.
        if !chunk_coords_in_range(req.chunk_x, req.chunk_z) {
            return self.replies().error(
                request_id,
                "bad_request",
                format!(
                    "chunk ({}, {}) is outside the representable range of +/-{MAX_CHUNK_COORD} chunks",
                    req.chunk_x, req.chunk_z
                ),
            );
        }
        let block_x = tiles::chunk_min_block(req.chunk_x);
        let block_z = tiles::chunk_min_block(req.chunk_z);
        let anchor = match self.resolve_anchor(req.anchor_id, block_x, block_z) {
            Ok(anchor) => anchor,
            Err((code, message)) => return self.replies().error(request_id, code, message),
        };
        let key = TileKey::from_chunk(anchor.id, req.chunk_x, req.chunk_z, self.tile_size());

        {
            let mut cache = self.shared.tiles();
            if let Some(tile) = cache.get(key) {
                self.shared
                    .ctx
                    .status
                    .cache_hits
                    .fetch_add(1, Ordering::Relaxed);
                self.shared
                    .send_chunk(request_id, tile, req.chunk_x, req.chunk_z);
                return;
            }
        }

        self.shared
            .ctx
            .status
            .cache_misses
            .fetch_add(1, Ordering::Relaxed);
        let Some(cancel) = self.shared.in_flight.claim(request_id) else {
            return self
                .replies()
                .error(request_id, "busy", "too many requests in flight");
        };
        self.enqueue_tile(
            key,
            anchor,
            Waiter::Chunk {
                request_id,
                chunk_x: req.chunk_x,
                chunk_z: req.chunk_z,
                cancel,
            },
            false,
        );
    }

    fn handle_request_column(&self, request_id: u64, req: RequestColumn) {
        // A block coordinate near `i32::MIN` overflows when its tile rect is grown by the
        // margin, deep inside the job, so it is rejected here instead. See `MAX_BLOCK_COORD`.
        if !block_coords_in_range(req.x, req.z) {
            return self.replies().error(
                request_id,
                "bad_request",
                format!(
                    "block ({}, {}) is outside the representable range of +/-{MAX_BLOCK_COORD} blocks",
                    req.x, req.z
                ),
            );
        }
        let anchor = match self.resolve_anchor(req.anchor_id, req.x, req.z) {
            Ok(anchor) => anchor,
            Err((code, message)) => return self.replies().error(request_id, code, message),
        };
        let key = TileKey::from_block(anchor.id, req.x, req.z, self.tile_size());

        {
            let mut cache = self.shared.tiles();
            if let Some(tile) = cache.get(key) {
                self.shared
                    .ctx
                    .status
                    .cache_hits
                    .fetch_add(1, Ordering::Relaxed);
                self.shared.send_column(request_id, tile, req.x, req.z);
                return;
            }
        }

        self.shared
            .ctx
            .status
            .cache_misses
            .fetch_add(1, Ordering::Relaxed);
        let Some(cancel) = self.shared.in_flight.claim(request_id) else {
            return self
                .replies()
                .error(request_id, "busy", "too many requests in flight");
        };
        self.enqueue_tile(
            key,
            anchor,
            Waiter::Column {
                request_id,
                x: req.x,
                z: req.z,
                cancel,
            },
            false,
        );
    }

    /// Warm the tiles around a point. No reply on success, and the hint is dropped rather than
    /// queued when the worker is busy with requests a client is actually blocked on.
    fn handle_prefetch(&self, request_id: u64, req: Prefetch) {
        // Checked once, before the loop: the `saturating_add` below keeps every offset inside
        // the range this admits, so nothing in the loop can overflow.
        if !chunk_coords_in_range(req.chunk_x, req.chunk_z) {
            return self.replies().error(
                request_id,
                "bad_request",
                format!(
                    "chunk ({}, {}) is outside the representable range of +/-{MAX_CHUNK_COORD} chunks",
                    req.chunk_x, req.chunk_z
                ),
            );
        }
        if let Some(id) = req.anchor_id {
            if self.anchors().get(id).is_none() {
                // A hint gets no reply, but an anchor id that does not exist is a client
                // bookkeeping bug and is worth saying out loud.
                return self.replies().error(
                    request_id,
                    "unknown_anchor",
                    format!("anchor id {id} was never registered in this session"),
                );
            }
        }

        let radius = req
            .radius_chunks
            .unwrap_or(DEFAULT_PREFETCH_RADIUS)
            .min(MAX_PREFETCH_RADIUS) as i32;
        let tile_size = self.tile_size();

        // Walk the covered chunks but enqueue each distinct tile once: a 4-chunk radius over a
        // 512-block tile is one or two tiles, not 81 jobs.
        let mut seen: Vec<TileKey> = Vec::new();
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let chunk_x = req.chunk_x.saturating_add(dx);
                let chunk_z = req.chunk_z.saturating_add(dz);
                let block_x = tiles::chunk_min_block(chunk_x);
                let block_z = tiles::chunk_min_block(chunk_z);
                // Prefetching across a patch edge is normal; skip the chunks that fall outside.
                let Ok(anchor) = self.resolve_anchor(req.anchor_id, block_x, block_z) else {
                    continue;
                };
                let key = TileKey::from_chunk(anchor.id, chunk_x, chunk_z, tile_size);
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
                // Bound to a local: a guard created inside an `if` condition would still be held
                // through the body, and `enqueue_tile` can take the same lock again.
                let already_cached = self.shared.tiles().contains(&key);
                if !already_cached {
                    self.enqueue_tile(key, anchor, Waiter::Prefetch, true);
                }
            }
        }
    }

    /// `Cancel` flags the named request (or all of them) and, for each tile job whose whole
    /// audience has now gone, flags the job too so the pipeline can stop at its next stage
    /// boundary. A job with one interested client left keeps going.
    fn handle_cancel(&self, req: Cancel) {
        self.shared.in_flight.cancel(req.request_id);
        let mut pending = self.shared.pending();
        for entry in pending.values_mut() {
            if !entry.waiters.is_empty() && entry.waiters.iter().all(Waiter::is_cancelled) {
                entry.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Size a world for an area. Runs on the worker because it fetches elevation data over the
    /// network, but it does not consume an in-flight slot: it is a sizing question a client asks
    /// once, before it has a world at all.
    fn handle_elevation_range(&self, request_id: u64, req: QueryElevationRange) {
        let bbox = match bbox_around(req.lat, req.lon, req.radius_m) {
            Ok(bbox) => bbox,
            Err(reason) => return self.replies().error(request_id, "bad_request", reason),
        };

        let replies = self.replies().clone();
        let panic_replies = replies.clone();
        let run = move || {
            replies.progress(
                request_id,
                "fetching_elevation",
                format!("sampling {:.4}, {:.4}", req.lat, req.lon),
            );
            match crate::elevation::query_elevation_range(&bbox) {
                Ok((min_m, max_m)) => {
                    let (min_y, height, sea_level_y) = recommend_vertical(min_m, max_m);
                    replies.send(
                        &ServerMessage::JsonReply(JsonReply::ElevationRange(ElevationRangeReply {
                            min_elevation_m: min_m,
                            max_elevation_m: max_m,
                            recommended_min_y: min_y,
                            recommended_height: height,
                            recommended_sea_level_y: sea_level_y,
                        })),
                        request_id,
                    );
                }
                Err(e) => replies.error(
                    request_id,
                    "generation_failed",
                    format!("could not sample elevation for this area: {e}"),
                ),
            }
        };
        let on_panic = move |detail: String| {
            panic_replies.error(
                request_id,
                "generation_failed",
                format!("the elevation query panicked: {detail}"),
            );
        };
        let busy_replies = self.replies().clone();
        let on_busy = move || {
            busy_replies.error(
                request_id,
                "busy",
                "Arnis is generating a world right now; retry this request when it finishes",
            );
        };

        let job = Job {
            label: format!("elevation range at {:.4}, {:.4}", req.lat, req.lon),
            run: Box::new(run),
            on_panic: Box::new(on_panic),
            on_busy: Box::new(on_busy),
        };
        if let Err(e) = self.shared.ctx.jobs.submit(job) {
            self.replies().error(request_id, e.code(), e.message());
        }
    }

    /// Place a new anchor and add it to the session's set.
    fn handle_add_anchor(&self, request_id: u64, req: AddAnchor) {
        let radius = req.radius_m.unwrap_or(DEFAULT_ANCHOR_RADIUS_M);
        if !(radius.is_finite() && radius > 0.0 && radius <= MAX_ANCHOR_RADIUS_M) {
            return self.replies().error(
                request_id,
                "bad_request",
                format!("radiusM must be greater than 0 and at most {MAX_ANCHOR_RADIUS_M}"),
            );
        }

        let anchor = {
            let mut anchors = self.anchors();
            let scale = self.shared.basis.args.scale;
            let mut anchor = match anchors.place_new(req.lat, req.lon, radius, scale) {
                Ok(anchor) => anchor,
                Err(reason) => return self.replies().error(request_id, "bad_request", reason),
            };
            // `place_new` assigns the lowest free id; a client that wants its own id gets it,
            // and `insert` rejects it if it collides or overlaps.
            if let Some(id) = req.id {
                anchor.id = id;
            }
            if let Err(reason) = anchors.insert(anchor) {
                return self.replies().error(request_id, "bad_request", reason);
            }
            self.shared.refresh_job_config(&anchors);
            anchor
        };

        self.publish_anchors();
        self.shared.ctx.status.log(format!(
            "Anchor {} placed at {:.5}, {:.5} -> block {},{} (radius {:.0} m).",
            anchor.id, anchor.lat, anchor.lon, anchor.mc_x, anchor.mc_z, anchor.radius_m
        ));
        self.replies().send(
            &ServerMessage::JsonReply(JsonReply::Anchor(AnchorReply {
                id: anchor.id,
                lat: anchor.lat,
                lon: anchor.lon,
                mc_x: anchor.mc_x,
                mc_z: anchor.mc_z,
                radius_m: anchor.radius_m,
            })),
            request_id,
        );
    }

    /// Resolve a location to block coordinates.
    ///
    /// A coordinate pair is answered exactly. A place *name* is not: answering one needs a
    /// place-name index, and the only acceptable source for that is a local OSM extract, which
    /// stream mode does not have yet. Arnis deliberately does not call Nominatim or any other
    /// online geocoder here — a chunk server making third-party web requests on a player's
    /// behalf is not something to add quietly — so this is reported as the documented limitation
    /// `geocoding_unavailable` rather than papered over.
    fn handle_locate(&self, request_id: u64, req: Locate) {
        let Some((lat, lon)) = parse_lat_lon(&req.query) else {
            return self.replies().error(
                request_id,
                "geocoding_unavailable",
                "Place-name search is not available: Arnis has no place-name index in stream \
                 mode and does not call an online geocoding service. Send coordinates instead, \
                 as \"lat, lon\" (for example \"54.63, 9.93\"). Names become answerable once a \
                 local .osm.pbf extract can be configured.",
            );
        };

        let (anchor_id, mc_x, mc_z, note) = match self.anchors().find_containing_latlon(lat, lon) {
            Some(anchor) => {
                let (x, z) = anchor.projection().forward(lat, lon);
                (
                    Some(anchor.id),
                    Some(x.round() as i32),
                    Some(z.round() as i32),
                    format!("Inside the patch of anchor {}.", anchor.id),
                )
            }
            None => (
                None,
                None,
                None,
                "These coordinates lie outside every registered patch. Create an anchor for them \
                 with AddAnchor."
                    .to_string(),
            ),
        };

        self.replies().send(
            &ServerMessage::JsonReply(JsonReply::Locate(LocateReply {
                found: true,
                lat,
                lon,
                anchor_id,
                mc_x,
                mc_z,
                note,
            })),
            request_id,
        );
    }
}

// -------------------------------------------------------------------------------------------
// Chunk helpers
// -------------------------------------------------------------------------------------------

/// The all-air payload for a chunk the tile never generated.
fn air_chunk(chunk_x: i32, chunk_z: i32, clipped: bool) -> ChunkPayload {
    ChunkPayload {
        chunk_x,
        chunk_z,
        clipped,
        min_section_y: 0,
        sections: Vec::new(),
        biomes: std::array::from_fn(|_| DEFAULT_BIOME.to_string()),
        block_entities: Vec::new(),
    }
}

/// Whether a blockstate string names one of the air blocks.
///
/// Compared as a prefix so a hypothetical state-carrying air block still counts; today none of
/// them have properties, but a `starts_with` costs nothing and cannot be wrong in the other
/// direction, because no non-air block id starts with one of these names.
fn is_air_state(state: &str) -> bool {
    ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"]
        .iter()
        .any(|air| state.starts_with(air))
}

/// Y of the highest non-air block in one column of an encoded chunk, or `None` when the column
/// is entirely air.
///
/// Reads the encoded payload rather than the world tree, because by the time a column probe is
/// answered the tile has already been encoded and thrown the world away. Sections are scanned
/// from the top down, so the loop stops at the first solid block it meets.
fn surface_y_in(payload: &ChunkPayload, x: i32, z: i32) -> Option<i32> {
    let local_x = x.rem_euclid(16) as usize;
    let local_z = z.rem_euclid(16) as usize;

    for (index, section) in payload.sections.iter().enumerate().rev() {
        let section_base = (payload.min_section_y + index as i32) * 16;
        match section {
            SectionPayload::Empty => continue,
            SectionPayload::Uniform(state) => {
                if !is_air_state(state) {
                    return Some(section_base + 15);
                }
            }
            SectionPayload::Paletted { palette, indices } => {
                for y in (0..16usize).rev() {
                    // Cell order is YZX, matching the wire format.
                    let cell = y * 256 + local_z * 16 + local_x;
                    let Some(&palette_index) = indices.get(cell) else {
                        continue;
                    };
                    let Some(state) = palette.get(palette_index as usize) else {
                        continue;
                    };
                    if !is_air_state(state) {
                        return Some(section_base + y as i32);
                    }
                }
            }
        }
    }
    None
}

// -------------------------------------------------------------------------------------------
// Small helpers
// -------------------------------------------------------------------------------------------

/// Parse `"lat, lon"` or `"lat lon"`. Anything else is a place name, which stream mode cannot
/// resolve.
fn parse_lat_lon(query: &str) -> Option<(f64, f64)> {
    let cleaned = query.replace(',', " ");
    let mut parts = cleaned.split_whitespace();
    let lat: f64 = parts.next()?.parse().ok()?;
    let lon: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    Some((lat, lon))
}

/// A square-ish bounding box of `radius_m` around a point.
///
/// The longitude span is widened by `1/cos(latitude)` so the box is as wide on the ground as it
/// is tall; without that, a query far from the equator would sample a sliver.
fn bbox_around(lat: f64, lon: f64, radius_m: f64) -> Result<LLBBox, String> {
    if !lat.is_finite() || !lon.is_finite() {
        return Err(format!("lat/lon must be finite, got ({lat}, {lon})"));
    }
    if !(radius_m.is_finite() && radius_m > 0.0) {
        return Err(format!("radiusM must be greater than 0, got {radius_m}"));
    }
    let d_lat = (radius_m / EARTH_RADIUS).to_degrees();
    // Guard the cosine: at the pole the longitude span is unbounded, so clamp it instead.
    let cos_lat = lat.to_radians().cos().abs().max(1e-4);
    let d_lon = (d_lat / cos_lat).min(179.0);

    let min_lat = (lat - d_lat).max(-89.9);
    let max_lat = (lat + d_lat).min(89.9);
    let min_lon = (lon - d_lon).max(-180.0);
    let max_lon = (lon + d_lon).min(180.0);
    LLBBox::new(min_lat, min_lon, max_lat, max_lon)
}

/// Round down to a multiple of 16.
fn floor16(v: i32) -> i32 {
    v.div_euclid(16) * 16
}

/// Round up to a multiple of 16.
fn ceil16(v: i32) -> i32 {
    floor16(v + 15)
}

/// Recommend a vertical mapping that covers a real elevation range, as
/// `(minY, height, seaLevelY)`.
///
/// Everything is computed in "blocks relative to sea level" first, then the whole window is slid
/// until it fits the engine. `seaLevelY = 0` is preferred and comes out whenever the terrain
/// fits under the ceiling with it, because that is the mapping where block Y simply *is* the
/// altitude in metres — no offset to teach players, and OSM `ele` tags need no conversion. Only
/// genuinely alpine terrain forces sea level off zero.
///
/// The result always satisfies [`VerticalMapping::validate`], so a client can send it straight
/// back in `Hello`.
fn recommend_vertical(min_m: f64, max_m: f64) -> (i32, i32, i32) {
    let low_m = if min_m.is_finite() { min_m } else { 0.0 };
    let high_m = if max_m.is_finite() { max_m } else { 0.0 };

    // Room below for the bedrock plane and the filled column, room above for buildings and trees.
    let mut low = floor16((low_m.floor() as i32).saturating_sub(RECOMMENDED_FLOOR_MARGIN));
    let mut high = ceil16((high_m.ceil() as i32).saturating_add(RECOMMENDED_HEADROOM));

    // Sea level has to be a real block strictly inside the world, so keep one section of the
    // window on each side of it even for terrain that never goes near sea level.
    low = low.min(-16);
    high = high.max(16);

    if high - low > MAX_WORLD_HEIGHT {
        // Taller than any Minecraft world can be. Keep the ground and clip the summits; the
        // clipping is reported per chunk, so the client can tell the player.
        high = low + MAX_WORLD_HEIGHT;
        if high < 16 {
            high = 16;
            low = high - MAX_WORLD_HEIGHT;
        }
    }

    // `sea` is where 0 m lands in block Y. Both bounds are multiples of 16 and, because the span
    // above is capped at exactly the engine's own range, `sea_lo <= sea_hi` always holds.
    let sea_hi = ENGINE_TOP - high;
    let sea_lo = ENGINE_FLOOR - low;
    let sea = 0.clamp(sea_lo.min(sea_hi), sea_hi);

    (sea + low, high - low, sea)
}

// -------------------------------------------------------------------------------------------
// Connection lifecycle
// -------------------------------------------------------------------------------------------

/// Serve one client from accept to close. Runs on its own thread, spawned by the accept loop.
pub(crate) fn serve_connection(stream: TcpStream, ctx: Arc<ServerContext>) {
    let peer = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown peer".to_string());

    // Replies are small and latency-sensitive — `Ping` exists so a client can measure round-trip
    // time — so Nagle's algorithm has nothing useful to coalesce and only adds delay.
    let _ = stream.set_nodelay(true);

    // Registered so that shutting the server down can half-close the socket and wake this thread
    // out of a blocking read. There is deliberately no read timeout: a timeout firing in the
    // middle of a frame would already have consumed part of it, and those bytes cannot be put
    // back — the stream would silently desynchronise.
    let connection_id = ctx.register_connection(&stream);

    let writer_socket = match stream.try_clone() {
        Ok(socket) => socket,
        Err(e) => {
            ctx.status
                .log(format!("Could not split the socket for {peer}: {e}"));
            ctx.unregister_connection(connection_id);
            return;
        }
    };

    let (tx, rx) = channel::<Frame>();
    let writer = match std::thread::Builder::new()
        .name("arnis-stream-writer".to_string())
        .spawn(move || writer_loop(writer_socket, rx))
    {
        Ok(handle) => handle,
        Err(e) => {
            ctx.status
                .log(format!("Could not start the writer thread for {peer}: {e}"));
            ctx.unregister_connection(connection_id);
            return;
        }
    };

    let replies = Replies {
        tx,
        status: Arc::clone(&ctx.status),
    };
    // `replies` is moved in, so once this returns the only senders left belong to jobs still on
    // the worker queue. The writer therefore lives exactly as long as there is anything to say.
    //
    // Caught rather than allowed to unwind the thread: the cleanup below is what removes this
    // socket from the shutdown registry and gives back the session count, and a panic that
    // skipped it would leak a duplicated file descriptor for the life of the process and leave
    // the GUI reporting a client that is long gone.
    let handshook = AtomicBool::new(false);
    if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
        run_connection(stream, &ctx, replies, &peer, &handshook)
    })) {
        ctx.status.log(format!(
            "Session for {peer} ended with a panic: {}",
            crate::stream::panic_message(payload.as_ref())
        ));
    }
    let handshook = handshook.load(Ordering::SeqCst);

    ctx.unregister_connection(connection_id);
    // Only the LAST session may clear the shared client status. This runs on every path past
    // the writer spawn, handshake failures and instant EOFs included, so clearing it
    // unconditionally would let any local process that opens the port and closes it again blank
    // out the connected client's name for the rest of its session.
    if handshook && ctx.live_sessions.fetch_sub(1, Ordering::SeqCst) == 1 {
        ctx.status.client_connected.store(false, Ordering::Relaxed);
        ctx.status.clear_client();
    }
    ctx.status.log(format!("Client {peer} disconnected."));

    // Bounded by the one tile job that may still be running: shutdown drops the jobs that are
    // merely queued rather than running them.
    let _ = writer.join();
}

/// The frame loop: handshake, then dispatch until the client closes or the server stops.
///
/// `handshook` is set once the handshake completes, i.e. once this connection counts towards
/// [`ServerContext::live_sessions`] and owes that counter a decrement. It is an out-parameter
/// rather than a return value so the caller still learns the truth if this unwinds.
fn run_connection(
    mut socket: TcpStream,
    ctx: &Arc<ServerContext>,
    replies: Replies,
    peer: &str,
    handshook: &AtomicBool,
) {
    let Some(first) = read_next(&mut socket, ctx) else {
        return;
    };

    let hello = match protocol::decode_client(&first) {
        Ok(ClientMessage::Hello(hello)) => hello,
        Ok(_) => {
            replies.error(
                first.request_id,
                "bad_request",
                "the first message on a connection must be Hello",
            );
            return;
        }
        Err(reason) => {
            replies.error(first.request_id, "bad_request", reason);
            return;
        }
    };

    let Some(session) = accept_hello(&hello, ctx, &replies, first.request_id) else {
        return;
    };
    handshook.store(true, Ordering::SeqCst);
    ctx.status.log(format!(
        "Client {peer} ({} {}) completed the handshake with {} anchor(s).",
        hello.client_name,
        hello.client_version,
        session.anchors().len()
    ));

    loop {
        let Some(frame) = read_next(&mut socket, ctx) else {
            return;
        };
        let request_id = frame.request_id;
        let message = match protocol::decode_client(&frame) {
            Ok(message) => message,
            Err(reason) => {
                replies.error(request_id, "bad_request", reason);
                continue;
            }
        };

        match message {
            // A second Hello is a client bug: the session's settings are already fixed, and
            // silently adopting new ones would make old chunks disagree with new ones.
            ClientMessage::Hello(_) => replies.error(
                request_id,
                "bad_request",
                "Hello may only be sent once, as the first message on a connection",
            ),
            // Answered here rather than on the worker, so it measures liveness, not load.
            ClientMessage::Ping => replies.send(&ServerMessage::Pong, request_id),
            ClientMessage::Cancel(req) => session.handle_cancel(req),
            ClientMessage::QueryElevationRange(req) => {
                session.handle_elevation_range(request_id, req)
            }
            ClientMessage::AddAnchor(req) => session.handle_add_anchor(request_id, req),
            ClientMessage::Locate(req) => session.handle_locate(request_id, req),
            ClientMessage::RequestChunk(req) => session.handle_request_chunk(request_id, req),
            ClientMessage::RequestColumn(req) => session.handle_request_column(request_id, req),
            ClientMessage::Prefetch(req) => session.handle_prefetch(request_id, req),
        }
    }
}

/// Read one frame, or `None` when the connection is over.
///
/// A clean close, a half-close from server shutdown and a reset client all end the loop the same
/// way; only a framing violation on a live server is worth a log line.
fn read_next(socket: &mut TcpStream, ctx: &Arc<ServerContext>) -> Option<Frame> {
    match protocol::read_frame(socket) {
        Ok(frame) => {
            if ctx.shutdown.load(Ordering::Relaxed) {
                None
            } else {
                Some(frame)
            }
        }
        Err(e) => {
            let expected = ctx.shutdown.load(Ordering::Relaxed)
                || matches!(
                    e.kind(),
                    ErrorKind::UnexpectedEof
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::BrokenPipe
                        | ErrorKind::NotConnected
                );
            if !expected {
                ctx.status.log(format!("Dropping connection: {e}"));
            }
            None
        }
    }
}

/// Validate `Hello` and, if it passes, build the session.
///
/// The order is fixed by the protocol: version, then token, then vertical mapping, then anchors.
/// It matters — a client on the wrong protocol version should be told that, not told its anchors
/// are malformed because they were parsed against a different schema.
fn accept_hello(
    hello: &Hello,
    ctx: &Arc<ServerContext>,
    replies: &Replies,
    request_id: u64,
) -> Option<Session> {
    if hello.protocol_version != protocol::PROTOCOL_VERSION {
        replies.hello_error(
            request_id,
            "version_mismatch",
            format!(
                "This client speaks stream protocol version {}, and Arnis {} speaks version {}. \
                 Update whichever of the two is older.",
                hello.protocol_version,
                env!("CARGO_PKG_VERSION"),
                protocol::PROTOCOL_VERSION
            ),
        );
        return None;
    }

    // A plain comparison on purpose. The token is not a security boundary — any local process
    // that can open this socket can also read the discovery file it came from — it exists to
    // turn "silently talking to the wrong Arnis process" into one clear error.
    if hello.session_token != ctx.session_token {
        replies.hello_error(
            request_id,
            "bad_token",
            "The session token does not match this Arnis instance. Re-read the token from the \
             stream.json discovery file in the Arnis configuration folder; a stale one usually \
             means Arnis was restarted since the client last read it.",
        );
        return None;
    }

    if let Err(reason) = hello.vertical.validate() {
        replies.hello_error(
            request_id,
            "invalid_vertical_mapping",
            format!("The world's vertical mapping is not usable: {reason}."),
        );
        return None;
    }

    let anchors: Vec<Anchor> = hello
        .anchors
        .iter()
        .map(|spec| {
            // Every anchor of a session carries that session's blocks-per-metre factor: the
            // anchor is the real-world-to-world mapping, and that mapping is scale-dependent.
            Anchor::new(
                spec.id,
                spec.lat,
                spec.lon,
                spec.mc_x,
                spec.mc_z,
                spec.radius_m,
                hello.config.scale,
            )
        })
        .collect();
    let anchors = match AnchorSet::new(anchors) {
        Ok(anchors) => anchors,
        Err(reason) => {
            replies.hello_error(
                request_id,
                "invalid_anchors",
                format!("The anchor set is not usable: {reason}."),
            );
            return None;
        }
    };

    // THE WORLD BOUNDS ARE DELIBERATELY NOT TOUCHED HERE. They are process globals, and this
    // runs on the connection's own thread: writing them from here retunes the dimension under a
    // tile job that the single worker may be halfway through, or under a world generation the
    // user started in the main window. The validated mapping is stored on the session instead
    // (`JobBasis::world_min_y`/`world_height`) and applied by `tiles::generate_tile`, on the
    // worker, holding the process-wide generation slot. One writer, one thread, no race.

    replies.send(
        &ServerMessage::HelloOk(HelloOk {
            arnis_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: protocol::PROTOCOL_VERSION,
            tile_size: ctx.config.tile_size.max(0) as u32,
            max_in_flight: MAX_IN_FLIGHT,
            // Capabilities name optional behaviour a client can branch on. `localOsmFile` is
            // here because a client can meaningfully check for it; geocoding is deliberately
            // absent, which is how a client learns not to send place names to `Locate`.
            capabilities: vec!["localOsmFile".to_string()],
        }),
        request_id,
    );

    ctx.status
        .set_client(&hello.client_name, &hello.client_version);
    ctx.status.client_connected.store(true, Ordering::Relaxed);
    ctx.status.set_activity("Connected");
    // Counted so that a second connection ending — a port scan, a failed handshake, a client
    // that reconnected — does not clear the status of the client still attached.
    ctx.live_sessions.fetch_add(1, Ordering::SeqCst);

    let basis = Arc::new(JobBasis {
        args: Arc::new(args_from(&hello.config, &hello.vertical)),
        vertical: AbsoluteVerticalMapping {
            sea_level_y: hello.vertical.sea_level_y,
            blocks_per_meter: hello.vertical.vertical_scale,
        },
        world_min_y: hello.vertical.min_y,
        world_height: hello.vertical.height,
        local_osm_file: hello.config.local_osm_file.clone(),
        tile_size: ctx.config.tile_size,
        margin: ctx.config.margin,
    });
    let job_config = Arc::new(basis.build(&anchors));
    let mut cache = TileCache::new(ctx.config.cache_tiles);
    cache.set_config_hash(tiles::config_hash(&job_config));

    let session = Session {
        shared: Shared {
            ctx: Arc::clone(ctx),
            replies: replies.clone(),
            basis,
            job_config: Arc::new(Mutex::new(job_config)),
            tiles: Arc::new(Mutex::new(cache)),
            pending: Arc::new(Mutex::new(FnvHashMap::default())),
            in_flight: Arc::new(InFlight::new(Arc::clone(&ctx.status))),
        },
        anchors: Arc::new(Mutex::new(anchors)),
    };
    session.publish_anchors();
    Some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_parse_and_place_names_do_not() {
        assert_eq!(parse_lat_lon("54.63, 9.93"), Some((54.63, 9.93)));
        assert_eq!(parse_lat_lon("  -12.5   77.25 "), Some((-12.5, 77.25)));
        assert_eq!(parse_lat_lon("Arnis, Germany"), None);
        assert_eq!(parse_lat_lon("54.63"), None);
        assert_eq!(parse_lat_lon("54.63, 9.93, 1.0"), None);
        // Out of range is a name, not a coordinate.
        assert_eq!(parse_lat_lon("91.0, 9.93"), None);
    }

    #[test]
    fn section_rounding_goes_the_right_way_for_negatives() {
        assert_eq!(floor16(0), 0);
        assert_eq!(floor16(-1), -16);
        assert_eq!(floor16(-16), -16);
        assert_eq!(ceil16(1), 16);
        assert_eq!(ceil16(16), 16);
        assert_eq!(ceil16(-1), 0);
    }

    /// Every recommendation has to be something the client can send straight back in `Hello`.
    fn assert_valid(min_y: i32, height: i32, sea: i32) {
        VerticalMapping {
            min_y,
            height,
            sea_level_y: sea,
            vertical_scale: 1.0,
        }
        .validate()
        .expect("recommendation must satisfy the handshake constraints");
    }

    #[test]
    fn lowland_terrain_gets_sea_level_zero() {
        // Northern Germany: a few metres of relief, so Y can read as metres.
        let (min_y, height, sea) = recommend_vertical(-2.0, 40.0);
        assert_eq!(sea, 0, "lowland terrain must keep Y equal to altitude");
        assert!(min_y < -2, "room below the lowest ground for the floor");
        assert!(min_y + height > 40 + RECOMMENDED_HEADROOM - 16);
        assert_valid(min_y, height, sea);
    }

    #[test]
    fn alpine_terrain_slides_sea_level_down_to_fit() {
        // Mont Blanc is 4809 m: above the 2031 ceiling but not above the maximum world height,
        // so the window slides down and sea level leaves zero.
        let (min_y, height, sea) = recommend_vertical(300.0, 4809.0);
        assert!(sea < 0, "the window has to slide down to fit the summits");
        assert!(min_y + height <= ENGINE_TOP);
        assert_valid(min_y, height, sea);
    }

    #[test]
    fn a_range_taller_than_any_world_is_clipped_not_rejected() {
        let (min_y, height, sea) = recommend_vertical(-500.0, 9000.0);
        assert_eq!(height, MAX_WORLD_HEIGHT);
        assert_valid(min_y, height, sea);
    }

    #[test]
    fn flat_terrain_still_leaves_a_section_on_each_side_of_sea_level() {
        let (min_y, height, sea) = recommend_vertical(0.0, 0.0);
        assert_eq!(sea, 0);
        assert!(min_y < 0 && min_y + height > 0);
        assert_valid(min_y, height, sea);
    }

    #[test]
    fn a_bbox_around_a_point_is_square_on_the_ground() {
        let bbox = bbox_around(54.63, 9.93, 5_000.0).expect("valid bbox");
        let lat_span = bbox.max().lat() - bbox.min().lat();
        let lon_span = bbox.max().lng() - bbox.min().lng();
        // At 54.6 N a degree of longitude covers ~0.58 of the ground a degree of latitude does,
        // so the longitude span has to be correspondingly wider for a square box.
        assert!(lon_span > lat_span * 1.5, "{lon_span} vs {lat_span}");
    }

    #[test]
    fn a_bbox_needs_a_finite_positive_radius() {
        assert!(bbox_around(54.63, 9.93, 0.0).is_err());
        assert!(bbox_around(54.63, 9.93, f64::NAN).is_err());
        assert!(bbox_around(f64::INFINITY, 9.93, 100.0).is_err());
    }

    #[test]
    fn air_states_are_recognised_and_solid_ones_are_not() {
        assert!(is_air_state("minecraft:air"));
        assert!(is_air_state("minecraft:cave_air"));
        assert!(is_air_state("minecraft:void_air"));
        assert!(!is_air_state("minecraft:stone"));
        assert!(!is_air_state("minecraft:oak_stairs[facing=north]"));
    }

    #[test]
    fn a_surface_probe_finds_the_highest_solid_block() {
        // Two sections starting at Y 0: a solid one, then one with a single block at y = 3
        // of the upper section, i.e. absolute Y 19.
        let mut indices = vec![0u16; 4096];
        let column = 3 * 256 + 5 * 16 + 7; // y=3, z=5, x=7
        indices[column] = 1;
        let payload = ChunkPayload {
            chunk_x: 0,
            chunk_z: 0,
            clipped: false,
            min_section_y: 0,
            sections: vec![
                SectionPayload::Uniform("minecraft:stone".to_string()),
                SectionPayload::Paletted {
                    palette: vec!["minecraft:air".to_string(), "minecraft:oak_log".to_string()],
                    indices,
                },
            ],
            biomes: std::array::from_fn(|_| DEFAULT_BIOME.to_string()),
            block_entities: Vec::new(),
        };

        assert_eq!(surface_y_in(&payload, 7, 5), Some(19));
        // A column with only air above the stone falls back to the top of the stone section.
        assert_eq!(surface_y_in(&payload, 0, 0), Some(15));
    }

    /// The regression: a tile every waiter cancelled must not answer the NEXT request
    /// `cancelled` and then never generate.
    #[test]
    fn a_new_request_revives_a_tile_whose_waiters_all_cancelled() {
        fn chunk_waiter(request_id: u64, cancelled: bool) -> Waiter {
            Waiter::Chunk {
                request_id,
                chunk_x: 0,
                chunk_z: 0,
                cancel: Arc::new(AtomicBool::new(cancelled)),
            }
        }

        let mut entry = PendingTile {
            cancel: Arc::new(AtomicBool::new(false)),
            waiters: vec![chunk_waiter(1, true)],
        };
        // What `handle_cancel` does once every waiter is cancelled.
        assert!(entry.waiters.iter().all(Waiter::is_cancelled));
        entry.cancel.store(true, Ordering::Relaxed);

        // A brand-new request for the same tile.
        entry.coalesce(chunk_waiter(2, false));
        assert!(
            !entry.cancel.load(Ordering::Relaxed),
            "a live waiter must revive the tile instead of inheriting a stale cancellation"
        );
        assert_eq!(entry.waiters.len(), 2);

        // A waiter that is itself already cancelled does not revive anything.
        entry.cancel.store(true, Ordering::Relaxed);
        entry.coalesce(chunk_waiter(3, true));
        assert!(entry.cancel.load(Ordering::Relaxed));
    }

    /// The stream window's "busy" indicator reads `requests_in_flight`, so every claim and every
    /// release has to move it — including the release that never happens because the connection
    /// died with requests outstanding.
    #[test]
    fn the_in_flight_counter_tracks_claims_releases_and_a_dropped_connection() {
        let status = Arc::new(StreamStatus::new(0));
        let count = || status.requests_in_flight.load(Ordering::Relaxed);

        {
            let in_flight = InFlight::new(Arc::clone(&status));
            assert_eq!(count(), 0);

            in_flight.claim(1).expect("a free slot");
            in_flight.claim(2).expect("a free slot");
            assert_eq!(count(), 2, "each claimed slot is one outstanding request");

            // A request that completes and one that fails both go through `release`.
            in_flight.release(1);
            in_flight.release(2);
            assert_eq!(count(), 0, "the counter must return to zero");

            // Releasing something unknown must not push it below zero.
            in_flight.release(99);
            assert_eq!(count(), 0);

            // A reused request id replaces its slot; it must not be counted twice, or the
            // single release that follows would strand the counter above zero.
            in_flight.claim(7).expect("a free slot");
            in_flight.claim(7).expect("a free slot");
            assert_eq!(count(), 1);
            in_flight.release(7);
            assert_eq!(count(), 0);

            // Now leave requests outstanding and let the connection go.
            in_flight.claim(11).expect("a free slot");
            in_flight.claim(12).expect("a free slot");
            assert_eq!(count(), 2);
        }
        assert_eq!(
            count(),
            0,
            "a connection dropping with requests outstanding must not pin the indicator at busy"
        );
    }

    #[test]
    fn coordinates_beyond_the_representable_range_are_rejected() {
        assert!(chunk_coords_in_range(0, 0));
        assert!(chunk_coords_in_range(MAX_CHUNK_COORD, -MAX_CHUNK_COORD));
        assert!(!chunk_coords_in_range(MAX_CHUNK_COORD + 1, 0));
        assert!(!chunk_coords_in_range(0, i32::MIN));
        assert!(!chunk_coords_in_range(i32::MAX, 0));

        assert!(block_coords_in_range(0, 0));
        assert!(block_coords_in_range(MAX_BLOCK_COORD, -MAX_BLOCK_COORD));
        assert!(!block_coords_in_range(MAX_BLOCK_COORD + 1, 0));
        assert!(!block_coords_in_range(0, i32::MIN));

        // Everything the limits admit survives the multiply the handlers perform.
        assert_eq!(tiles::chunk_min_block(MAX_CHUNK_COORD), MAX_BLOCK_COORD);
        // A Minecraft world border is 29 999 984 blocks, so nothing a real client can reach is
        // refused.
        assert!(chunk_coords_in_range(1_874_999, -1_874_999));
    }

    #[test]
    fn an_all_air_chunk_has_no_surface() {
        let payload = air_chunk(4, -2, false);
        assert_eq!(surface_y_in(&payload, 4 * 16, -2 * 16), None);
        assert_eq!(payload.sections.len(), 0);
        assert_eq!(payload.biomes.len(), 16);
    }
}
