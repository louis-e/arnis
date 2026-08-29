//! Tile-size benchmark harness for stream mode.
//!
//! Stream mode never generates a single chunk: it generates a **tile** plus a **margin**, keeps
//! the inner tile and serves 16x16 chunks out of it. The tile size is therefore the one knob that
//! trades the two things a player notices against each other:
//!
//! * **Throughput.** A tile costs a fixed overhead (two HTTP fetches, the elevation grid, the
//!   flood-fill caches) plus a marginal cost proportional to the *padded* area it generates.
//!   Bigger tiles amortise the fixed cost over more usable area and waste proportionally less of
//!   the padded area on margin.
//! * **Time to first chunk.** A player walking into cold terrain waits for the whole tile. Bigger
//!   tiles make that wait longer.
//!
//! This module measures both, plus peak memory per in-flight tile, so the default can be argued
//! from numbers instead of taste. The written report lives in `docs/stream-tile-size.md`.
//!
//! # How it measures
//!
//! Through the real path, not a simulation. Each configuration starts an actual stream server on
//! an ephemeral loopback port, connects a TCP client, completes a real `Hello` handshake and then
//! asks for real chunks. Time to first chunk is the wall clock from writing `RequestChunk` to
//! reading the first `ChunkData` frame back off the socket, which is exactly the latency the mod
//! sees: the tile job runs to completion before the first waiter is answered.
//!
//! # How it is invoked
//!
//! There is no CLI flag — stream mode does not have one, and the benchmark is not a user-facing
//! feature. It is gated on the `ARNIS_STREAM_BENCH` environment variable, in the style of the
//! other `ARNIS_*` switches, and reports through the same `[BENCHMARK] <label>=<value>` stderr
//! lines as [`crate::bench`] so existing tooling parses it unchanged.
//!
//! Wiring is one line at the top of `main()`:
//!
//! ```ignore
//! if crate::stream::bench::run_from_env() {
//!     return;
//! }
//! ```
//!
//! Environment:
//!
//! | Variable | Meaning |
//! | --- | --- |
//! | `ARNIS_STREAM_BENCH=1` | Run the benchmark instead of a normal Arnis run. |
//! | `ARNIS_STREAM_BENCH_AREAS` | Comma-separated area slugs to run. Default: all of [`DEFAULT_AREAS`]. |
//! | `ARNIS_STREAM_BENCH_SIZES` | Comma-separated tile sizes. Default: [`DEFAULT_TILE_SIZES`]. |
//! | `ARNIS_STREAM_BENCH_WARM=1` | Run every configuration twice and keep the second, so the shared on-disk elevation/OSM caches are warm for the measured pass. |
//!
//! # What the numbers are worth
//!
//! * **The network is in the measurement.** A cold configuration pays Overpass and elevation
//!   fetches; a warm one hits the on-disk cache under `dirs::cache_dir()/arnis-tile-cache`, which
//!   is shared across runs. Compare sizes measured in the same state, and prefer
//!   `ARNIS_STREAM_BENCH_WARM=1` when the question is about generation rather than bandwidth.
//! * **Memory is process RSS.** Generation is serialised onto one worker, so during a measured
//!   request exactly one tile is in flight and the peak-minus-baseline delta is that tile's cost.
//!   The allocator does not return freed pages promptly, though, so the delta shrinks for every
//!   configuration after the first *in the same process*. Trust the memory column only from runs
//!   that measured a single configuration — that is what `ARNIS_STREAM_BENCH_AREAS` and
//!   `ARNIS_STREAM_BENCH_SIZES` are for.

use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use colored::Colorize;
use serde::Serialize;

use crate::stream::protocol::{
    self, AnchorSpec, ErrorMessage, Frame, GenConfig, Hello, HelloError, HelloOk, RequestChunk,
    VerticalMapping, MSG_CHUNK_DATA, MSG_ERROR, MSG_HELLO, MSG_HELLO_ERROR, MSG_HELLO_OK,
    MSG_PROGRESS, MSG_REQUEST_CHUNK, PROTOCOL_VERSION,
};
use crate::stream::{self, tiles, DiscoveryFile, StreamConfig};

/// Blocks along one edge of a Minecraft chunk. Private copy of the constant in
/// [`crate::stream::tiles`], which does not export it.
const CHUNK_BLOCKS: i32 = 16;

/// Enables the harness. Anything other than `1` leaves Arnis behaving normally.
const ENV_ENABLE: &str = "ARNIS_STREAM_BENCH";

/// Comma-separated area slugs to restrict the run to.
const ENV_AREAS: &str = "ARNIS_STREAM_BENCH_AREAS";

/// Comma-separated tile sizes to restrict the run to.
const ENV_SIZES: &str = "ARNIS_STREAM_BENCH_SIZES";

/// Run every configuration twice and report the second pass.
const ENV_WARM: &str = "ARNIS_STREAM_BENCH_WARM";

/// How often the memory sampler reads this process's RSS while a tile generates.
const RSS_SAMPLE_INTERVAL: Duration = Duration::from_millis(50);

/// Vertical mapping the harness declares: the vanilla 1.18+ overworld.
///
/// Fixed rather than derived per area, because a mapping that changed between areas would change
/// how much column each tile has to fill and make the areas incomparable.
const BENCH_MIN_Y: i32 = -64;
/// World height for [`BENCH_MIN_Y`]. Vanilla overworld.
const BENCH_HEIGHT: i32 = 384;
/// Block Y of 0 m elevation for [`BENCH_MIN_Y`]. Vanilla sea level.
const BENCH_SEA_LEVEL_Y: i32 = 64;

// ---------------------------------------------------------------------------------------------
// Areas
// ---------------------------------------------------------------------------------------------

/// How built-up an area is, which is what actually drives element count per square kilometre and
/// therefore how tile size behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaKind {
    /// City centre: dense building footprints, many small ways, 3D building parts.
    DenseCity,
    /// Detached housing on a street grid: moderate element count, large uniform ground areas.
    Suburban,
    /// Village, farmland or coast: few elements, terrain and land cover dominate.
    Rural,
}

impl AreaKind {
    /// Stable machine-readable name, used in metric labels and the report table.
    pub fn label(self) -> &'static str {
        match self {
            AreaKind::DenseCity => "dense_city",
            AreaKind::Suburban => "suburban",
            AreaKind::Rural => "rural",
        }
    }
}

/// One real place the benchmark generates at every tile size.
#[derive(Clone, Copy, Debug)]
pub struct BenchArea {
    /// Human-readable name, e.g. `"Manhattan Midtown"`.
    pub name: &'static str,
    /// Which density class this area stands in for.
    pub kind: AreaKind,
    /// WGS84 latitude of the anchor.
    pub lat: f64,
    /// WGS84 longitude of the anchor.
    pub lon: f64,
}

impl BenchArea {
    /// Lowercase underscore form of [`BenchArea::name`], used in metric labels.
    pub fn slug(&self) -> String {
        slugify(self.name)
    }
}

/// The three areas the report is built from: one per [`AreaKind`], all real places whose density
/// is easy to sanity-check on a map.
///
/// * Manhattan Midtown is about as dense as OpenStreetMap gets: skyscraper footprints, building
///   parts, a full street grid and heavy indoor tagging.
/// * Levittown is the archetypal post-war American suburb — uniform detached houses on curved
///   streets, which is the shape most "walk out of the city" streaming ends up in.
/// * Arnis is Germany's smallest town and this project's namesake: a few hundred buildings on a
///   fjord, so terrain, water and land cover dominate rather than vector elements.
pub const DEFAULT_AREAS: [BenchArea; 3] = [
    BenchArea {
        name: "Manhattan Midtown",
        kind: AreaKind::DenseCity,
        lat: 40.7549,
        lon: -73.9840,
    },
    BenchArea {
        name: "Levittown NY",
        kind: AreaKind::Suburban,
        lat: 40.7251,
        lon: -73.5143,
    },
    BenchArea {
        name: "Arnis Germany",
        kind: AreaKind::Rural,
        lat: 54.6300,
        lon: 9.9300,
    },
];

/// Tile sizes under test: two below the current default, the default, and one above.
///
/// These are the sizes `src/tile.rs` is now correct at — before that fix element assignment
/// silently dropped elements at every size but 512, so any measurement here would have been an
/// artefact of the bug rather than of the tile size.
pub const DEFAULT_TILE_SIZES: [i32; 4] = [128, 256, 512, 1024];

// ---------------------------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------------------------

/// One measured (area, tile size) configuration.
#[derive(Clone, Debug)]
pub struct BenchResult {
    /// [`BenchArea::name`] of the area measured.
    pub area: &'static str,
    /// Density class of that area.
    pub kind: AreaKind,
    /// Tile edge in blocks.
    pub tile_size: i32,
    /// Margin generated around the tile and discarded, in blocks.
    pub margin: i32,
    /// Chunks served out of the tile, i.e. `(tile_size / 16)^2`.
    pub chunks: u32,
    /// Compressed `ChunkData` payload bytes written for those chunks.
    pub bytes_served: u64,
    /// Wall clock from writing `RequestChunk` to reading the first `ChunkData` back. This is the
    /// whole tile job — fetches, generation and encoding — because the first waiter is answered
    /// only once the tile is complete.
    pub time_to_first_chunk_ms: f64,
    /// Wall clock for the first chunk plus every remaining chunk of the tile, requested one at a
    /// time. Everything after the first is a cache hit.
    pub total_ms: f64,
    /// Usable area of the tile in km², at scale 1.0 where one block is one metre.
    pub usable_km2: f64,
    /// Area actually generated, margin included, in km².
    pub padded_km2: f64,
    /// [`BenchResult::total_ms`] per usable km².
    pub ms_per_km2: f64,
    /// `padded_area / usable_area`. Pure geometry, identical for every area.
    pub margin_waste: f64,
    /// Highest process RSS observed while the tile was in flight, in bytes.
    pub peak_rss_bytes: u64,
    /// Peak RSS minus the RSS sampled immediately before the request: the cost of one in-flight
    /// tile, as far as the allocator lets it be seen. See the module note on memory.
    pub rss_delta_bytes: u64,
}

/// A least-squares fit of `time = fixed + marginal * padded_area` across the tile sizes measured
/// for one area.
///
/// The point of the fit is to separate what a generation job costs *at all* (fetch round trips,
/// elevation grid setup, cache construction) from what it costs *per unit of ground*. The first
/// term is what large tiles amortise away; the second is what the margin multiplies.
#[derive(Clone, Debug)]
pub struct CostModel {
    /// [`BenchArea::name`] this model was fitted for.
    pub area: &'static str,
    /// Intercept: milliseconds a generation job costs before it has generated any ground.
    pub fixed_ms: f64,
    /// Slope: milliseconds per padded km².
    pub marginal_ms_per_km2: f64,
    /// Coefficient of determination. A low value means the linear model does not describe this
    /// area and neither term should be quoted.
    pub r2: f64,
    /// Number of tile sizes the fit is based on.
    pub points: usize,
}

// ---------------------------------------------------------------------------------------------
// Pure geometry and fitting
// ---------------------------------------------------------------------------------------------

/// Lowercase, underscore-separated form of a name, for metric labels and env filters.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending_separator = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('_');
            }
            pending_separator = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_separator = true;
        }
    }
    out
}

/// Round a requested tile size to something stream mode can actually serve: at least one chunk,
/// and a whole number of chunks. Mirrors [`crate::stream::tiles::tile_size`].
pub fn normalise_tile_size(raw: i32) -> i32 {
    if raw < CHUNK_BLOCKS {
        return CHUNK_BLOCKS;
    }
    raw - raw.rem_euclid(CHUNK_BLOCKS)
}

/// Square kilometres covered by a square of `blocks` edge, at scale 1.0 (one block, one metre).
pub fn km2_of_square(blocks: i32) -> f64 {
    let km = f64::from(blocks) / 1000.0;
    km * km
}

/// Effective margin waste ratio: `padded_area / usable_area`, i.e. `(tile + 2 * margin)^2 /
/// tile^2`.
///
/// This is the argument for large tiles that needs no measurement at all. With the default
/// 128-block margin, a 128-block tile generates 384x384 to keep 128x128 — a factor of 9 — while a
/// 512-block tile generates 768x768 to keep 512x512, a factor of 2.25.
pub fn margin_waste(tile_size: i32, margin: i32) -> f64 {
    let padded = f64::from(tile_size) + 2.0 * f64::from(margin);
    let usable = f64::from(tile_size);
    (padded * padded) / (usable * usable)
}

/// Fit `time_to_first_chunk = fixed + marginal * padded_area` over one area's results.
///
/// Returns `None` when fewer than two tile sizes were measured, or when every measurement landed
/// on the same padded area (no slope is identifiable).
pub fn fit_cost_model(area: &str, results: &[BenchResult]) -> Option<CostModel> {
    let points: Vec<(f64, f64)> = results
        .iter()
        .filter(|r| r.area == area)
        .map(|r| (r.padded_km2, r.time_to_first_chunk_ms))
        .collect();
    if points.len() < 2 {
        return None;
    }

    let n = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;

    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for &(x, y) in &points {
        sxx += (x - mean_x) * (x - mean_x);
        sxy += (x - mean_x) * (y - mean_y);
    }
    if sxx <= 0.0 {
        return None;
    }

    let marginal_ms_per_km2 = sxy / sxx;
    let fixed_ms = mean_y - marginal_ms_per_km2 * mean_x;

    let mut ss_tot = 0.0;
    let mut ss_res = 0.0;
    for &(x, y) in &points {
        let predicted = fixed_ms + marginal_ms_per_km2 * x;
        ss_tot += (y - mean_y) * (y - mean_y);
        ss_res += (y - predicted) * (y - predicted);
    }
    let r2 = if ss_tot > 0.0 {
        1.0 - ss_res / ss_tot
    } else {
        1.0
    };

    let name = results.iter().find(|r| r.area == area).map(|r| r.area)?;
    Some(CostModel {
        area: name,
        fixed_ms,
        marginal_ms_per_km2,
        r2,
        points: points.len(),
    })
}

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Run the benchmark if `ARNIS_STREAM_BENCH=1`, returning whether it ran.
///
/// The caller is `main()`, which returns immediately when this returns `true`: a benchmark run
/// replaces the normal run rather than preceding it, because it retunes the process-global world
/// bounds that ordinary generation depends on.
pub fn run_from_env() -> bool {
    if !env_flag(ENV_ENABLE) {
        return false;
    }

    let areas = selected_areas();
    let sizes = selected_sizes();
    if areas.is_empty() || sizes.is_empty() {
        eprintln!(
            "{}",
            "[stream-bench] nothing to run: the area or size filter matched no configurations."
                .red()
        );
        return true;
    }

    eprintln!(
        "{}",
        format!(
            "[stream-bench] {} area(s) x {} tile size(s), margin {} blocks.",
            areas.len(),
            sizes.len(),
            tiles::margin()
        )
        .cyan()
    );

    let results = run_tile_size_benchmark(&areas, &sizes);
    report_cost_models(&results);
    print_markdown_table(&results);
    true
}

/// Measure every (area, tile size) combination, printing `[BENCHMARK]` lines as each completes.
///
/// A configuration that fails — no network, an Overpass timeout, a rejected handshake — is
/// reported and skipped rather than aborting the run, so one flaky area does not cost the whole
/// matrix. The returned vector therefore may be shorter than `areas.len() * sizes.len()`.
pub fn run_tile_size_benchmark(areas: &[BenchArea], sizes: &[i32]) -> Vec<BenchResult> {
    let warm = env_flag(ENV_WARM);
    let mut results = Vec::new();

    for area in areas {
        for &raw_size in sizes {
            let tile_size = normalise_tile_size(raw_size);
            eprintln!(
                "{}",
                format!("[stream-bench] {} at tile {tile_size}...", area.name).cyan()
            );

            if warm {
                match measure_once(area, tile_size) {
                    Ok(_) => eprintln!("  warm-up pass complete, discarded"),
                    Err(reason) => {
                        eprintln!("{}", format!("  warm-up pass failed: {reason}").red())
                    }
                }
            }

            match measure_once(area, tile_size) {
                Ok(result) => {
                    report_result(&result);
                    results.push(result);
                }
                Err(reason) => {
                    eprintln!("{}", format!("  FAILED: {reason}").red());
                }
            }
        }
    }

    results
}

/// Measure one configuration end to end: start a server, drive it, stop it again.
///
/// A fresh server per configuration is deliberate. The tile cache is per-server, so this is what
/// guarantees the measured request is a genuine cold first touch and not a cache hit left over
/// from the previous tile size.
fn measure_once(area: &BenchArea, tile_size: i32) -> Result<BenchResult, String> {
    let margin = tiles::margin();
    let handle = stream::start(StreamConfig {
        port: 0,
        tile_size,
        margin,
        cache_tiles: tiles::cache_tiles(),
    })?;
    let port = handle.port();

    let outcome = drive_session(area, tile_size, margin, port);

    handle.stop();
    outcome
}

/// The client half: handshake, one cold chunk, then the rest of the tile.
fn drive_session(
    area: &BenchArea,
    tile_size: i32,
    margin: i32,
    port: u16,
) -> Result<BenchResult, String> {
    let token = read_session_token()?;

    let socket = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("could not connect to the stream server on port {port}: {e}"))?;
    // The harness measures latency, so Nagle must not be allowed to sit on a request.
    let _ = socket.set_nodelay(true);
    let mut client = Client::new(socket)?;

    client.handshake(&token, area, tile_size)?;

    let chunks_per_side = tile_size / CHUNK_BLOCKS;
    let sampler = RssSampler::start();

    // The cold request. Everything the tile job costs is inside this one measurement.
    let started = Instant::now();
    let mut bytes_served = client.request_chunk(1, 0, 0)? as u64;
    let time_to_first_chunk = started.elapsed();

    // The rest of the tile, one request at a time. Sequential rather than pipelined so the
    // numbers stay unambiguous: the server answers cache hits inline on the reader thread, and a
    // pipelined window would fold queueing into the measurement without making it more realistic.
    let mut request_id = 2u64;
    for cz in 0..chunks_per_side {
        for cx in 0..chunks_per_side {
            if cx == 0 && cz == 0 {
                continue;
            }
            bytes_served += client.request_chunk(request_id, cx, cz)? as u64;
            request_id += 1;
        }
    }
    let total = started.elapsed();

    let (baseline_rss, peak_rss) = sampler.finish();

    let usable_km2 = km2_of_square(tile_size);
    let padded_km2 = km2_of_square(tile_size + 2 * margin);
    let total_ms = duration_ms(total);

    Ok(BenchResult {
        area: area.name,
        kind: area.kind,
        tile_size,
        margin,
        chunks: (chunks_per_side * chunks_per_side).max(0) as u32,
        bytes_served,
        time_to_first_chunk_ms: duration_ms(time_to_first_chunk),
        total_ms,
        usable_km2,
        padded_km2,
        ms_per_km2: if usable_km2 > 0.0 {
            total_ms / usable_km2
        } else {
            0.0
        },
        margin_waste: margin_waste(tile_size, margin),
        peak_rss_bytes: peak_rss,
        rss_delta_bytes: peak_rss.saturating_sub(baseline_rss),
    })
}

/// Milliseconds, as a float, so sub-millisecond cache hits do not all round to zero.
fn duration_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Patch radius that comfortably contains one tile.
///
/// A chunk request is rejected with `out_of_patch` when its column falls outside the anchor's
/// radius, and the anchor sits at the centre of tile (0, 0), so the radius only has to beat half
/// the tile diagonal. The extra kilometre is slack: the radius costs nothing to generate.
fn anchor_radius_m(tile_size: i32) -> f64 {
    f64::from(tile_size) * 2.0 + 1024.0
}

/// Read the session token this process just published in its discovery file.
///
/// Going through the file rather than through a back door on the handle is deliberate: it is the
/// same path a real client takes, so a broken discovery file fails the benchmark instead of
/// hiding behind it.
fn read_session_token() -> Result<String, String> {
    let path = stream::discovery_path()?;
    let body = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "could not read the stream discovery file at {}: {e}",
            path.display()
        )
    })?;
    let file: DiscoveryFile = serde_json::from_str(&body)
        .map_err(|e| format!("the stream discovery file is not valid JSON: {e}"))?;
    Ok(file.session_token)
}

// ---------------------------------------------------------------------------------------------
// Minimal protocol client
// ---------------------------------------------------------------------------------------------

/// A blocking stream-mode client, just complete enough to benchmark with.
///
/// Reads are buffered through a cloned socket while writes go to the original: `read_frame` reads
/// the header a field at a time, and an unbuffered socket would turn every frame into four
/// syscalls for no reason.
struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    fn new(socket: TcpStream) -> Result<Self, String> {
        let read_half = socket
            .try_clone()
            .map_err(|e| format!("could not split the benchmark client socket: {e}"))?;
        Ok(Self {
            reader: BufReader::new(read_half),
            writer: socket,
        })
    }

    /// Serialise `body` as JSON and write it as one frame.
    fn send_json<T: Serialize>(
        &mut self,
        msg_type: u8,
        request_id: u64,
        body: &T,
    ) -> Result<(), String> {
        let payload = serde_json::to_vec(body)
            .map_err(|e| format!("could not encode a benchmark request: {e}"))?;
        protocol::write_frame(
            &mut self.writer,
            &Frame {
                msg_type,
                request_id,
                payload,
            },
        )
        .map_err(|e| format!("could not write a benchmark request: {e}"))?;
        self.writer
            .flush()
            .map_err(|e| format!("could not flush a benchmark request: {e}"))
    }

    fn read_frame(&mut self) -> Result<Frame, String> {
        protocol::read_frame(&mut self.reader)
            .map_err(|e| format!("could not read a reply from the stream server: {e}"))
    }

    /// Complete the handshake, declaring one anchor centred on the area.
    fn handshake(
        &mut self,
        session_token: &str,
        area: &BenchArea,
        tile_size: i32,
    ) -> Result<HelloOk, String> {
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "arnis-tile-size-benchmark".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            session_token: session_token.to_string(),
            config: GenConfig {
                scale: 1.0,
                fillground: false,
                interior: false,
                use3d: false,
                overture: false,
                canopy_height: false,
                terrain_only: false,
                // The benchmark measures the real streaming path, terrain fetch included.
                flat_ground: false,
                local_osm_file: None,
            },
            vertical: VerticalMapping {
                min_y: BENCH_MIN_Y,
                height: BENCH_HEIGHT,
                sea_level_y: BENCH_SEA_LEVEL_Y,
                vertical_scale: 1.0,
            },
            // The anchor sits at the centre of tile (0, 0), so the area under test is the tile
            // the benchmark measures rather than the quadrant north-east of it.
            anchors: vec![AnchorSpec {
                id: 1,
                lat: area.lat,
                lon: area.lon,
                mc_x: tile_size / 2,
                mc_z: tile_size / 2,
                radius_m: anchor_radius_m(tile_size),
            }],
        };

        self.send_json(MSG_HELLO, 0, &hello)?;
        let frame = self.read_frame()?;
        match frame.msg_type {
            MSG_HELLO_OK => serde_json::from_slice::<HelloOk>(&frame.payload)
                .map_err(|e| format!("could not decode HelloOk: {e}")),
            MSG_HELLO_ERROR => {
                let error: HelloError = serde_json::from_slice(&frame.payload)
                    .map_err(|e| format!("could not decode HelloError: {e}"))?;
                Err(format!(
                    "handshake rejected ({}): {}",
                    error.code, error.reason
                ))
            }
            other => Err(format!(
                "expected HelloOk or HelloError, got message type {other}"
            )),
        }
    }

    /// Request one chunk and block until its `ChunkData` arrives, returning the payload size.
    ///
    /// `Progress` frames are skipped: they are informational and a cache hit emits none at all.
    fn request_chunk(
        &mut self,
        request_id: u64,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Result<usize, String> {
        self.send_json(
            MSG_REQUEST_CHUNK,
            request_id,
            &RequestChunk {
                chunk_x,
                chunk_z,
                anchor_id: Some(1),
            },
        )?;

        loop {
            let frame = self.read_frame()?;
            if frame.request_id != request_id {
                return Err(format!(
                    "expected a reply to request {request_id}, got one for {}",
                    frame.request_id
                ));
            }
            match frame.msg_type {
                MSG_PROGRESS => continue,
                MSG_CHUNK_DATA => return Ok(frame.payload.len()),
                MSG_ERROR => {
                    let error: ErrorMessage = serde_json::from_slice(&frame.payload)
                        .map_err(|e| format!("could not decode an Error reply: {e}"))?;
                    return Err(format!(
                        "chunk ({chunk_x}, {chunk_z}) failed with {}: {}",
                        error.code, error.message
                    ));
                }
                other => {
                    return Err(format!(
                        "unexpected message type {other} in reply to a chunk request"
                    ))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Memory sampling
// ---------------------------------------------------------------------------------------------

/// Samples this process's resident set size on a background thread while a tile is in flight.
///
/// Polling is the only option available without new dependencies, and it is enough: a tile job
/// takes seconds, so a 50 ms sample catches the peak comfortably.
struct RssSampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
    baseline: u64,
}

impl RssSampler {
    fn start() -> Self {
        let mut system = sysinfo::System::new();
        let baseline = process_rss_bytes(&mut system);

        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(baseline));

        let thread_stop = Arc::clone(&stop);
        let thread_peak = Arc::clone(&peak);
        let handle = thread::Builder::new()
            .name("arnis-stream-bench-rss".to_string())
            .spawn(move || {
                let mut system = sysinfo::System::new();
                while !thread_stop.load(Ordering::Relaxed) {
                    thread_peak.fetch_max(process_rss_bytes(&mut system), Ordering::Relaxed);
                    thread::sleep(RSS_SAMPLE_INTERVAL);
                }
                // One last sample, so a job that finishes between polls still counts.
                thread_peak.fetch_max(process_rss_bytes(&mut system), Ordering::Relaxed);
            })
            .ok();

        Self {
            stop,
            peak,
            handle,
            baseline,
        }
    }

    /// Stop sampling and return `(baseline, peak)` in bytes.
    fn finish(mut self) -> (u64, u64) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        (self.baseline, self.peak.load(Ordering::Relaxed))
    }
}

/// This process's resident set size in bytes, or 0 if the platform will not say.
fn process_rss_bytes(system: &mut sysinfo::System) -> u64 {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map_or(0, |process| process.memory())
}

// ---------------------------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------------------------

/// Print one `[BENCHMARK] <label>=<value>` line, in the format `src/bench.rs` established.
fn report_line(label: &str, value: impl std::fmt::Display) {
    eprintln!("[BENCHMARK] {label}={value}");
}

/// Metric label for one configuration, e.g. `stream_arnis_germany_t512_ttfc_ms`.
fn metric_label(result: &BenchResult, metric: &str) -> String {
    format!(
        "stream_{}_t{}_{metric}",
        slugify(result.area),
        result.tile_size
    )
}

/// Emit every metric for one measured configuration.
fn report_result(result: &BenchResult) {
    report_line(
        &metric_label(result, "ttfc_ms"),
        format!("{:.0}", result.time_to_first_chunk_ms),
    );
    report_line(
        &metric_label(result, "total_ms"),
        format!("{:.0}", result.total_ms),
    );
    report_line(
        &metric_label(result, "ms_per_km2"),
        format!("{:.0}", result.ms_per_km2),
    );
    report_line(&metric_label(result, "chunks"), result.chunks);
    report_line(&metric_label(result, "bytes_served"), result.bytes_served);
    report_line(
        &metric_label(result, "peak_rss_bytes"),
        result.peak_rss_bytes,
    );
    report_line(
        &metric_label(result, "rss_delta_bytes"),
        result.rss_delta_bytes,
    );
    report_line(
        &metric_label(result, "margin_waste"),
        format!("{:.3}", result.margin_waste),
    );
    report_line(
        &metric_label(result, "padded_km2"),
        format!("{:.4}", result.padded_km2),
    );
}

/// Fit and emit the fixed/marginal split for every area present in `results`.
fn report_cost_models(results: &[BenchResult]) {
    for area in distinct_areas(results) {
        let Some(model) = fit_cost_model(area, results) else {
            eprintln!(
                "{}",
                format!(
                    "[stream-bench] {area}: not enough tile sizes measured to split fixed from \
                     marginal cost."
                )
                .yellow()
            );
            continue;
        };
        let slug = slugify(model.area);
        report_line(
            &format!("stream_{slug}_fixed_ms"),
            format!("{:.0}", model.fixed_ms),
        );
        report_line(
            &format!("stream_{slug}_marginal_ms_per_km2"),
            format!("{:.0}", model.marginal_ms_per_km2),
        );
        report_line(&format!("stream_{slug}_fit_r2"), format!("{:.4}", model.r2));
        report_line(&format!("stream_{slug}_fit_points"), model.points);
    }
}

/// Area names in the order they were first measured.
fn distinct_areas(results: &[BenchResult]) -> Vec<&'static str> {
    let mut seen: Vec<&'static str> = Vec::new();
    for result in results {
        if !seen.contains(&result.area) {
            seen.push(result.area);
        }
    }
    seen
}

/// Print the results as a Markdown table, ready to paste into `docs/stream-tile-size.md` in place
/// of its placeholder rows.
fn print_markdown_table(results: &[BenchResult]) {
    if results.is_empty() {
        return;
    }
    eprintln!();
    eprintln!(
        "{}",
        "[stream-bench] paste into docs/stream-tile-size.md:".cyan()
    );
    eprintln!("| Area | Kind | Tile | Padded km² | Waste | Chunks | TTFC ms | Total ms | ms/km² | Peak RSS MB | Tile RSS MB |");
    eprintln!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for r in results {
        eprintln!(
            "| {} | {} | {} | {:.3} | {:.2} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |",
            r.area,
            r.kind.label(),
            r.tile_size,
            r.padded_km2,
            r.margin_waste,
            r.chunks,
            r.time_to_first_chunk_ms,
            r.total_ms,
            r.ms_per_km2,
            bytes_to_mb(r.peak_rss_bytes),
            bytes_to_mb(r.rss_delta_bytes),
        );
    }
}

/// Bytes as mebibytes, for human-readable output only.
fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

/// Whether an `ARNIS_*` switch is set to exactly `1`.
fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value.trim() == "1")
}

/// Areas to run: all of [`DEFAULT_AREAS`], or those whose slug or name matches `ARNIS_STREAM_BENCH_AREAS`.
fn selected_areas() -> Vec<BenchArea> {
    let Ok(raw) = std::env::var(ENV_AREAS) else {
        return DEFAULT_AREAS.to_vec();
    };
    let wanted: Vec<String> = raw
        .split(',')
        .map(slugify)
        .filter(|token| !token.is_empty())
        .collect();
    if wanted.is_empty() {
        return DEFAULT_AREAS.to_vec();
    }
    for token in &wanted {
        if !DEFAULT_AREAS.iter().any(|area| area.slug() == *token) {
            eprintln!(
                "{}",
                format!(
                    "[stream-bench] no area matches '{token}'; known areas: {}",
                    known_areas()
                )
                .yellow()
            );
        }
    }
    DEFAULT_AREAS
        .iter()
        .filter(|area| wanted.contains(&area.slug()))
        .copied()
        .collect()
}

/// Comma-separated list of the known area slugs, for error messages.
fn known_areas() -> String {
    DEFAULT_AREAS
        .iter()
        .map(BenchArea::slug)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tile sizes to run: [`DEFAULT_TILE_SIZES`], or the parsed `ARNIS_STREAM_BENCH_SIZES` list.
fn selected_sizes() -> Vec<i32> {
    let Ok(raw) = std::env::var(ENV_SIZES) else {
        return DEFAULT_TILE_SIZES.to_vec();
    };
    let sizes: Vec<i32> = raw
        .split(',')
        .filter_map(|token| token.trim().parse::<i32>().ok())
        .map(normalise_tile_size)
        .collect();
    if sizes.is_empty() {
        eprintln!(
            "{}",
            format!("[stream-bench] {ENV_SIZES} parsed to nothing; using the defaults.").yellow()
        );
        return DEFAULT_TILE_SIZES.to_vec();
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(area: &'static str, tile_size: i32, ttfc_ms: f64) -> BenchResult {
        let margin = 128;
        BenchResult {
            area,
            kind: AreaKind::Rural,
            tile_size,
            margin,
            chunks: ((tile_size / CHUNK_BLOCKS) * (tile_size / CHUNK_BLOCKS)) as u32,
            bytes_served: 0,
            time_to_first_chunk_ms: ttfc_ms,
            total_ms: ttfc_ms,
            usable_km2: km2_of_square(tile_size),
            padded_km2: km2_of_square(tile_size + 2 * margin),
            ms_per_km2: 0.0,
            margin_waste: margin_waste(tile_size, margin),
            peak_rss_bytes: 0,
            rss_delta_bytes: 0,
        }
    }

    // The geometric argument the report is built on: with the default 128 margin a 128 tile
    // generates nine times the ground it keeps, and a 512 tile only 2.25 times.
    #[test]
    fn margin_waste_matches_the_reported_ratios() {
        assert!((margin_waste(128, 128) - 9.0).abs() < 1e-9);
        assert!((margin_waste(256, 128) - 4.0).abs() < 1e-9);
        assert!((margin_waste(512, 128) - 2.25).abs() < 1e-9);
        assert!((margin_waste(1024, 128) - 1.5625).abs() < 1e-9);
        // No margin, no waste.
        assert!((margin_waste(512, 0) - 1.0).abs() < 1e-9);
    }

    // Areas are reported in km² at scale 1.0, where a block is a metre.
    #[test]
    fn square_kilometres_of_a_tile() {
        assert!((km2_of_square(1000) - 1.0).abs() < 1e-9);
        assert!((km2_of_square(512) - 0.262144).abs() < 1e-9);
    }

    // Tile edges must break on chunk boundaries, or a chunk would belong to two tiles.
    #[test]
    fn tile_sizes_are_rounded_to_whole_chunks() {
        assert_eq!(normalise_tile_size(0), 16);
        assert_eq!(normalise_tile_size(-100), 16);
        assert_eq!(normalise_tile_size(15), 16);
        assert_eq!(normalise_tile_size(17), 16);
        assert_eq!(normalise_tile_size(520), 512);
        assert_eq!(normalise_tile_size(512), 512);
    }

    // The fit has to recover a line it was given exactly, or the fixed/marginal split reported
    // in the docs means nothing.
    #[test]
    fn cost_model_recovers_a_known_line() {
        let fixed = 1500.0;
        let marginal = 4000.0;
        let results: Vec<BenchResult> = [128, 256, 512, 1024]
            .iter()
            .map(|&size| {
                let padded = km2_of_square(size + 256);
                result("Test Area", size, fixed + marginal * padded)
            })
            .collect();

        let model = fit_cost_model("Test Area", &results).expect("fit");
        assert!(
            (model.fixed_ms - fixed).abs() < 1e-6,
            "fixed {}",
            model.fixed_ms
        );
        assert!(
            (model.marginal_ms_per_km2 - marginal).abs() < 1e-6,
            "marginal {}",
            model.marginal_ms_per_km2
        );
        assert!(model.r2 > 0.999_999);
        assert_eq!(model.points, 4);
    }

    // One measurement cannot separate a fixed cost from a marginal one.
    #[test]
    fn cost_model_needs_at_least_two_sizes() {
        let results = vec![result("Test Area", 512, 1000.0)];
        assert!(fit_cost_model("Test Area", &results).is_none());
        assert!(fit_cost_model("Missing Area", &results).is_none());
    }

    // Results from several areas must not contaminate each other's fits.
    #[test]
    fn cost_model_only_uses_its_own_area() {
        let mut results: Vec<BenchResult> = [256, 512]
            .iter()
            .map(|&size| result("A", size, 1000.0 + 2000.0 * km2_of_square(size + 256)))
            .collect();
        results.push(result("B", 512, 999_999.0));

        let model = fit_cost_model("A", &results).expect("fit");
        assert_eq!(model.points, 2);
        assert!((model.fixed_ms - 1000.0).abs() < 1e-6);
        assert_eq!(distinct_areas(&results), vec!["A", "B"]);
    }

    // Metric labels are what the `[BENCHMARK]` lines are grepped by, so they must be stable and
    // free of anything a shell would choke on.
    #[test]
    fn metric_labels_are_plain_identifiers() {
        assert_eq!(slugify("Manhattan Midtown"), "manhattan_midtown");
        assert_eq!(slugify("Arnis, Germany"), "arnis_germany");
        assert_eq!(slugify("  spaced  out  "), "spaced_out");
        assert_eq!(
            metric_label(&result("Levittown NY", 512, 0.0), "ttfc_ms"),
            "stream_levittown_ny_t512_ttfc_ms"
        );
    }

    // The report claims one area per density class, at plausible real coordinates.
    #[test]
    fn default_areas_cover_every_kind() {
        for kind in [AreaKind::DenseCity, AreaKind::Suburban, AreaKind::Rural] {
            assert!(
                DEFAULT_AREAS.iter().any(|area| area.kind == kind),
                "no default area of kind {}",
                kind.label()
            );
        }
        for area in &DEFAULT_AREAS {
            assert!((-90.0..=90.0).contains(&area.lat), "{} lat", area.name);
            assert!((-180.0..=180.0).contains(&area.lon), "{} lon", area.name);
            assert!(!area.slug().is_empty());
        }
    }

    // The anchor sits at the tile centre, so its radius must contain the whole tile: the furthest
    // chunk column is half a diagonal away.
    #[test]
    fn anchor_radius_contains_the_whole_tile() {
        for size in DEFAULT_TILE_SIZES {
            let half_diagonal = f64::from(size) * std::f64::consts::SQRT_2 / 2.0;
            assert!(
                anchor_radius_m(size) > half_diagonal,
                "radius {} does not contain tile {size}",
                anchor_radius_m(size)
            );
        }
    }
}
