//! Wire protocol for Arnis stream mode.
//!
//! Stream mode turns Arnis into a small local TCP server that a game client (typically a
//! Minecraft Fabric mod on `127.0.0.1`) talks to. This module owns everything that touches the
//! wire and nothing else: framing, the JSON control-plane message shapes, and the compact binary
//! chunk encoding. It performs no I/O of its own beyond reading and writing the byte streams it
//! is handed, so it can be unit-tested end to end without a socket.
//!
//! # Framing
//!
//! Every message in both directions is one frame:
//!
//! ```text
//! [u32 payload_len][u8 msg_type][u64 request_id][payload]
//! ```
//!
//! All integers are **little endian**. `payload_len` counts only the payload bytes, never the
//! thirteen header bytes, so a frame occupies `payload_len + 13` bytes on the wire.
//! `request_id` is
//! chosen by the client and echoed by the server on every message belonging to that request.
//!
//! # Two planes
//!
//! * **Control plane** — every message except [`MSG_CHUNK_DATA`] carries a UTF-8 JSON payload.
//!   JSON is not the fast path, so its cost does not matter, and being able to read a session in
//!   a packet dump is worth far more than the bytes it saves.
//! * **Data plane** — `ChunkData` (130) alone carries a compact binary body, DEFLATE-compressed.
//!   It is the only message sent in bulk and the only one worth encoding tightly.
//!
//! Message type 131 (`ColumnData`) is, despite its name, the **generic JSON reply**: it answers
//! `QueryElevationRange`, `AddAnchor`, `Locate` and `RequestColumn`. Every 131 body carries a
//! `kind` discriminator so a client can parse it without consulting its own request table;
//! correlation still happens through `request_id`, and `kind` exists so that a mismatch between
//! the two is detectable rather than silently misparsed. See [`JsonReply`].
//!
//! The normative specification is `docs/stream-protocol.md`.

// The protocol surface is defined in full here, including the decoder half that only tests and
// third-party clients use. Without this the unused half trips `dead_code` under `-D warnings`.
#![allow(dead_code)]

use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

/// Protocol version Arnis speaks. A client sending a different version is rejected at handshake
/// with `HelloError { code: "version_mismatch" }`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest payload accepted or produced, in bytes.
///
/// The length prefix arrives before any of the data it describes, so an unchecked prefix would
/// let a peer make Arnis allocate an arbitrary amount of memory with four bytes of input.
/// Anything above this is treated as a framing violation: the stream cannot be resynchronised
/// after a bad length, so the connection is closed rather than skipped forward.
pub const MAX_PAYLOAD: u32 = 64 * 1024 * 1024;

/// Largest chunk body accepted *after* decompression.
///
/// DEFLATE expands, so a payload under [`MAX_PAYLOAD`] can still inflate to something enormous.
/// Decompression is bounded by this so a hostile or corrupt peer cannot turn a small frame into
/// an out-of-memory abort.
const MAX_CHUNK_RAW: u64 = 64 * 1024 * 1024;

/// Cells in one 16x16x16 section.
const SECTION_CELLS: usize = 4096;

/// Smallest number of bytes one block entity can occupy in a chunk body.
///
/// Three `i32` coordinates plus two `[u16 len][utf8]` strings with empty bodies. Used to reject a
/// block-entity count that the remaining bytes cannot possibly describe, before reserving for it.
const BLOCK_ENTITY_MIN_BYTES: usize = 16;

/// Lowest legal [`VerticalMapping::min_y`], the mirror of the 2032 ceiling.
///
/// Block Y is packed into a signed 12-bit field, so the engine's absolute range is -2048..2047;
/// with one section of lighting headroom reserved below the floor, -2032 is the deepest a world
/// may start. A floor below this passes every other constraint yet pushes the Anvil section index
/// `(y >> 4)` outside the signed byte the section maps use, where it wraps and silently files
/// blocks into completely wrong sections.
pub const ENGINE_FLOOR: i32 = -2032;

// ---------------------------------------------------------------------------------------------
// Message type bytes
// ---------------------------------------------------------------------------------------------

/// `Hello` (client -> server): the handshake. Must be the first message on a connection.
pub const MSG_HELLO: u8 = 1;
/// `QueryElevationRange` (client -> server): sample real elevation before sizing a world.
pub const MSG_QUERY_ELEVATION_RANGE: u8 = 2;
/// `AddAnchor` (client -> server): register a new anchor mid-session.
pub const MSG_ADD_ANCHOR: u8 = 3;
/// `RequestChunk` (client -> server): ask for one 16x16 chunk column.
pub const MSG_REQUEST_CHUNK: u8 = 4;
/// `RequestColumn` (client -> server): cheap surface-height probe for one block column.
pub const MSG_REQUEST_COLUMN: u8 = 5;
/// `Locate` (client -> server): geocode a place name and locate it in block coordinates.
pub const MSG_LOCATE: u8 = 6;
/// `Prefetch` (client -> server): hint that chunks around a point will be needed soon.
pub const MSG_PREFETCH: u8 = 7;
/// `Cancel` (client -> server): cancel one in-flight request, or all of them.
pub const MSG_CANCEL: u8 = 8;
/// `Ping` (client -> server): liveness probe, answered on the reader path.
pub const MSG_PING: u8 = 9;

/// `HelloOk` (server -> client): handshake accepted.
pub const MSG_HELLO_OK: u8 = 128;
/// `HelloError` (server -> client): handshake rejected; the connection closes after it.
pub const MSG_HELLO_ERROR: u8 = 129;
/// `ChunkData` (server -> client): the DEFLATE-compressed chunk body. See [`encode_chunk`].
pub const MSG_CHUNK_DATA: u8 = 130;
/// `ColumnData` (server -> client): the generic JSON reply. See [`JsonReply`].
pub const MSG_JSON_REPLY: u8 = 131;
/// `Progress` (server -> client): optional progress notes before a request's terminal message.
pub const MSG_PROGRESS: u8 = 132;
/// `Error` (server -> client): terminal failure for one request. Never closes the connection.
pub const MSG_ERROR: u8 = 133;
/// `Pong` (server -> client): reply to `Ping`.
pub const MSG_PONG: u8 = 134;

// ---------------------------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------------------------

/// One framed message: the thirteen-byte header plus its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Message type byte, one of the `MSG_*` constants.
    pub msg_type: u8,
    /// Client-chosen correlation id, echoed by the server on every message for that request.
    pub request_id: u64,
    /// Raw payload bytes: UTF-8 JSON, except for [`MSG_CHUNK_DATA`] which is raw DEFLATE.
    pub payload: Vec<u8>,
}

/// Read exactly one frame from `r`, blocking until it is complete.
///
/// Returns [`io::ErrorKind::InvalidData`] if the length prefix exceeds [`MAX_PAYLOAD`]; the
/// oversized length is rejected *before* the payload buffer is allocated. A stream that ends
/// mid-frame yields [`io::ErrorKind::UnexpectedEof`].
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Frame> {
    let payload_len = r.read_u32::<LittleEndian>()?;
    if payload_len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame payload of {payload_len} bytes exceeds the {MAX_PAYLOAD} byte limit"),
        ));
    }
    let msg_type = r.read_u8()?;
    let request_id = r.read_u64::<LittleEndian>()?;
    let mut payload = vec![0u8; payload_len as usize];
    r.read_exact(&mut payload)?;
    Ok(Frame {
        msg_type,
        request_id,
        payload,
    })
}

/// Write one frame to `w`. Does not flush; the caller owns the buffering policy.
///
/// Returns [`io::ErrorKind::InvalidData`] if the payload is larger than [`MAX_PAYLOAD`], so an
/// internal bug cannot put a frame on the wire that no conforming peer would accept.
pub fn write_frame<W: Write>(w: &mut W, frame: &Frame) -> io::Result<()> {
    if frame.payload.len() > MAX_PAYLOAD as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "frame payload of {} bytes exceeds the {MAX_PAYLOAD} byte limit",
                frame.payload.len()
            ),
        ));
    }
    w.write_u32::<LittleEndian>(frame.payload.len() as u32)?;
    w.write_u8(frame.msg_type)?;
    w.write_u64::<LittleEndian>(frame.request_id)?;
    w.write_all(&frame.payload)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Control plane: client -> server
// ---------------------------------------------------------------------------------------------

/// Generation settings, fixed for the whole session by the handshake.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenConfig {
    /// Blocks per metre. `1.0` is real size.
    pub scale: f64,
    /// Fill the column below the surface with stone instead of leaving it hollow.
    pub fillground: bool,
    /// Generate building interiors.
    pub interior: bool,
    /// Use 3D building data (roof shapes, building parts) where available.
    pub use3d: bool,
    /// Additionally fetch Overture Maps data.
    pub overture: bool,
    /// Use canopy-height raster data for tree heights.
    pub canopy_height: bool,
    /// Terrain only: skip OpenStreetMap and Overture entirely.
    pub terrain_only: bool,
    /// Objects on flat ground: skip elevation, land cover and canopy entirely.
    ///
    /// Every other mode fetches raster data for each tile, so this is the only configuration that
    /// generates without touching the network once vector data is local. Terrain is flat at
    /// `seaLevelY`; the vertical mapping still applies to everything built on top of it.
    /// Mutually exclusive with `terrainOnly`, which would leave nothing to generate.
    #[serde(default)]
    pub flat_ground: bool,
    /// Absolute path to a local OSM extract. When set, tile generation reads vector data from
    /// this file instead of querying Overpass. This is the seam an offline `.osm.pbf` reader
    /// plugs into: all vector fetching goes through one function, so the file path can replace
    /// the network path without any other change.
    #[serde(default)]
    pub local_osm_file: Option<String>,
}

/// How the client's world maps real elevation onto block Y.
///
/// The mapping is absolute, never relative to the terrain in view:
/// `y = seaLevelY + round(elevation_m * verticalScale)`. Two tiles generated hours apart
/// therefore agree on height and join seamlessly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerticalMapping {
    /// Lowest block Y of the client's world.
    pub min_y: i32,
    /// Total vertical extent in blocks.
    pub height: i32,
    /// Block Y that represents 0 m elevation.
    pub sea_level_y: i32,
    /// Blocks per metre of real elevation. `1.0` is real relief.
    pub vertical_scale: f64,
}

impl VerticalMapping {
    /// Check the mapping against the engine limits.
    ///
    /// These are hard limits of the target engine, not tuning parameters, and the server never
    /// clamps a bad mapping into a good one — by the time the handshake arrives the client has
    /// already created its world, and a silently different mapping would produce terrain that
    /// does not match what is already on disk.
    ///
    /// * `minY % 16 == 0` and `height % 16 == 0` — the unit of storage is a 16x16x16 section, so
    ///   a floor or ceiling in the middle of a section corrupts every index derived from it.
    /// * `height <= 4064` — the vertical section index is a signed byte (sections -128..127) and
    ///   one section of headroom is reserved at each end for lighting, leaving 254 x 16 blocks.
    /// * `minY + height <= 2032` — block positions are packed into a 64-bit integer with 12 bits
    ///   for Y, so the highest legal block Y is 2031.
    /// * `minY >= -2032` ([`ENGINE_FLOOR`]) — the mirror of the ceiling, and the constraint that
    ///   anchors the section span. `height <= 4064` bounds how *many* sections a world spans but
    ///   says nothing about where that span sits, so a deeper floor still fits every other check
    ///   while `(y >> 4)` overflows the signed byte the section maps are keyed by: a floor of
    ///   -4048 gives section -253, which wraps to 3, and `world_section_range()` then reports a
    ///   minimum above its maximum. Blocks land in wrapped sections and the client receives
    ///   terrain with instructions to place it thousands of blocks from where it belongs, so this
    ///   is rejected at the handshake rather than clamped.
    /// * `minY < seaLevelY < minY + height` — sea level must be a real block inside the world,
    ///   strictly inside, so that there is at least one block above and below it.
    /// * `verticalScale` finite and `> 0` — zero or negative scale would collapse or invert all
    ///   relief, and a non-finite scale poisons every derived height.
    pub fn validate(&self) -> Result<(), String> {
        if self.min_y % 16 != 0 {
            return Err(format!("minY ({}) must be a multiple of 16", self.min_y));
        }
        if self.height % 16 != 0 {
            return Err(format!("height ({}) must be a multiple of 16", self.height));
        }
        if self.height <= 0 {
            return Err(format!("height ({}) must be positive", self.height));
        }
        if self.height > 4064 {
            return Err(format!("height ({}) must be at most 4064", self.height));
        }
        // After the height check on purpose: a world that is both too deep and too tall should
        // report the height first, which is the constraint the client can actually act on.
        if self.min_y < ENGINE_FLOOR {
            return Err(format!(
                "minY ({}) must be at least {ENGINE_FLOOR} (the lowest legal world floor; \
                 a deeper floor wraps the signed-byte section index)",
                self.min_y
            ));
        }
        let top = self
            .min_y
            .checked_add(self.height)
            .ok_or_else(|| "minY + height overflows".to_string())?;
        if top > 2032 {
            return Err(format!(
                "minY + height ({top}) must be at most 2032 (highest legal block Y is 2031)"
            ));
        }
        if self.sea_level_y <= self.min_y || self.sea_level_y >= top {
            return Err(format!(
                "seaLevelY ({}) must lie strictly between minY ({}) and minY + height ({top})",
                self.sea_level_y, self.min_y
            ));
        }
        if !self.vertical_scale.is_finite() || self.vertical_scale <= 0.0 {
            return Err(format!(
                "verticalScale ({}) must be finite and greater than 0",
                self.vertical_scale
            ));
        }
        Ok(())
    }
}

/// A real-world anchor point and the patch of world it owns, as the client persisted it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorSpec {
    /// Client-side anchor id, unique within the session.
    pub id: u32,
    /// WGS84 latitude in degrees.
    pub lat: f64,
    /// WGS84 longitude in degrees.
    pub lon: f64,
    /// Block X the anchor's lat/lon maps to.
    pub mc_x: i32,
    /// Block Z the anchor's lat/lon maps to.
    pub mc_z: i32,
    /// Patch radius in metres.
    pub radius_m: f64,
}

/// `Hello` (1): the handshake, and the only message that may open a connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// Protocol version the client speaks.
    pub protocol_version: u32,
    /// Free-form client identifier, logged by the server.
    pub client_name: String,
    /// Client's own version string, logged by the server.
    pub client_version: String,
    /// Must equal the `sessionToken` from the discovery file.
    pub session_token: String,
    /// Generation settings for the whole session.
    pub config: GenConfig,
    /// Vertical mapping for the whole session.
    pub vertical: VerticalMapping,
    /// Anchors the client already holds. Taken as given, not recomputed; may be empty.
    pub anchors: Vec<AnchorSpec>,
}

/// `QueryElevationRange` (2): sample real elevation around a point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryElevationRange {
    /// Centre latitude.
    pub lat: f64,
    /// Centre longitude.
    pub lon: f64,
    /// Radius in metres to sample.
    pub radius_m: f64,
}

/// `AddAnchor` (3): register a new anchor mid-session.
///
/// Unlike [`AnchorSpec`] this carries no `mcX`/`mcZ`: deriving where the anchor belongs is the
/// server's job, and the reply carries the placement the client must persist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAnchor {
    /// Client-chosen id. When absent the server assigns the lowest unused id.
    #[serde(default)]
    pub id: Option<u32>,
    /// WGS84 latitude.
    pub lat: f64,
    /// WGS84 longitude.
    pub lon: f64,
    /// Patch radius in metres. When absent the server picks its default.
    #[serde(default)]
    pub radius_m: Option<f64>,
}

/// `RequestChunk` (4): ask for one 16x16 chunk column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestChunk {
    /// Chunk X, i.e. `block_x >> 4`.
    pub chunk_x: i32,
    /// Chunk Z, i.e. `block_z >> 4`.
    pub chunk_z: i32,
    /// Patch to resolve against. When absent the server resolves the containing patch.
    #[serde(default)]
    pub anchor_id: Option<u32>,
}

/// `RequestColumn` (5): cheap surface probe for one block column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestColumn {
    /// Block X.
    pub x: i32,
    /// Block Z.
    pub z: i32,
    /// Patch to resolve against. When absent the server resolves the containing patch.
    #[serde(default)]
    pub anchor_id: Option<u32>,
}

/// `Locate` (6): geocode a place name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Locate {
    /// Free-form place name, e.g. `"Arnis, Germany"`.
    pub query: String,
}

/// `Prefetch` (7): a hint that chunks around a point will be wanted soon.
///
/// Produces no reply on success and may be ignored entirely; a client must never wait on one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefetch {
    /// Centre chunk X.
    pub chunk_x: i32,
    /// Centre chunk Z.
    pub chunk_z: i32,
    /// Chebyshev radius in chunks to warm. When absent the server picks its default.
    #[serde(default)]
    pub radius_chunks: Option<u32>,
    /// Patch to resolve against. When absent the server resolves the containing patch.
    #[serde(default)]
    pub anchor_id: Option<u32>,
}

/// `Cancel` (8): cancel in-flight work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cancel {
    /// The in-flight request to cancel. When absent, every in-flight request on the connection
    /// is cancelled. Note this is a *field* naming another request; the `Cancel` frame's own
    /// header `request_id` is its own and is only used to report a `bad_request` against it.
    #[serde(default)]
    pub request_id: Option<u64>,
}

/// Any message a client can send, already parsed from its frame.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    /// `Hello` (1).
    Hello(Hello),
    /// `QueryElevationRange` (2).
    QueryElevationRange(QueryElevationRange),
    /// `AddAnchor` (3).
    AddAnchor(AddAnchor),
    /// `RequestChunk` (4).
    RequestChunk(RequestChunk),
    /// `RequestColumn` (5).
    RequestColumn(RequestColumn),
    /// `Locate` (6).
    Locate(Locate),
    /// `Prefetch` (7).
    Prefetch(Prefetch),
    /// `Cancel` (8).
    Cancel(Cancel),
    /// `Ping` (9). Carries an empty JSON object.
    Ping,
}

/// Parse a client frame.
///
/// The error is a human-readable reason suitable for an `Error { code: "bad_request" }` message;
/// an unknown message type byte is a `bad_request` too, not a fatal framing error.
pub fn decode_client(frame: &Frame) -> Result<ClientMessage, String> {
    /// Parse a JSON payload, naming the message in the error.
    fn parse<T: for<'de> Deserialize<'de>>(name: &str, payload: &[u8]) -> Result<T, String> {
        serde_json::from_slice(payload).map_err(|e| format!("invalid {name} payload: {e}"))
    }

    match frame.msg_type {
        MSG_HELLO => Ok(ClientMessage::Hello(parse("Hello", &frame.payload)?)),
        MSG_QUERY_ELEVATION_RANGE => Ok(ClientMessage::QueryElevationRange(parse(
            "QueryElevationRange",
            &frame.payload,
        )?)),
        MSG_ADD_ANCHOR => Ok(ClientMessage::AddAnchor(parse(
            "AddAnchor",
            &frame.payload,
        )?)),
        MSG_REQUEST_CHUNK => Ok(ClientMessage::RequestChunk(parse(
            "RequestChunk",
            &frame.payload,
        )?)),
        MSG_REQUEST_COLUMN => Ok(ClientMessage::RequestColumn(parse(
            "RequestColumn",
            &frame.payload,
        )?)),
        MSG_LOCATE => Ok(ClientMessage::Locate(parse("Locate", &frame.payload)?)),
        MSG_PREFETCH => Ok(ClientMessage::Prefetch(parse("Prefetch", &frame.payload)?)),
        MSG_CANCEL => Ok(ClientMessage::Cancel(parse("Cancel", &frame.payload)?)),
        MSG_PING => {
            // Ping carries `{}`; anything that parses as a JSON object is accepted, so that a
            // future field on Ping does not break older servers.
            let _: serde_json::Value = parse("Ping", &frame.payload)?;
            Ok(ClientMessage::Ping)
        }
        other => Err(format!("unknown message type {other}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Control plane: server -> client
// ---------------------------------------------------------------------------------------------

/// `HelloOk` (128): handshake accepted, with the session's fixed parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloOk {
    /// Arnis version, e.g. `"3.1.0"`.
    pub arnis_version: String,
    /// Protocol version the server speaks.
    pub protocol_version: u32,
    /// Tile edge in blocks. Always a multiple of 16.
    pub tile_size: u32,
    /// Maximum concurrent generation-bearing requests before the server answers `busy`.
    pub max_in_flight: u32,
    /// Optional feature names. This is the forward-compatibility channel within a protocol
    /// version; clients must ignore entries they do not recognise and must not require any.
    pub capabilities: Vec<String>,
}

/// `HelloError` (129): handshake rejected. The connection closes after it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloError {
    /// Human-readable explanation, suitable for showing to a user.
    pub reason: String,
    /// One of `"version_mismatch"`, `"bad_token"`, `"invalid_vertical_mapping"`,
    /// `"invalid_anchors"`.
    pub code: String,
}

/// Reply to [`QueryElevationRange`], carried on [`MSG_JSON_REPLY`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevationRangeReply {
    /// Lowest sampled elevation in metres.
    pub min_elevation_m: f64,
    /// Highest sampled elevation in metres.
    pub max_elevation_m: f64,
    /// Suggested [`VerticalMapping::min_y`], already section-aligned and within limits.
    pub recommended_min_y: i32,
    /// Suggested [`VerticalMapping::height`].
    pub recommended_height: i32,
    /// Suggested [`VerticalMapping::sea_level_y`].
    pub recommended_sea_level_y: i32,
}

/// Reply to [`AddAnchor`], carried on [`MSG_JSON_REPLY`]. The client persists this verbatim and
/// sends it back as an [`AnchorSpec`] in the next session's [`Hello`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorReply {
    /// Assigned or echoed anchor id.
    pub id: u32,
    /// Echoed latitude.
    pub lat: f64,
    /// Echoed longitude.
    pub lon: f64,
    /// Derived block X of the anchor.
    pub mc_x: i32,
    /// Derived block Z of the anchor.
    pub mc_z: i32,
    /// Effective patch radius in metres.
    pub radius_m: f64,
}

/// Reply to [`Locate`], carried on [`MSG_JSON_REPLY`].
///
/// `found: false` is a successful reply, not an error — the query simply matched nothing. A
/// resolved place outside every patch comes back with `anchor_id: None`, which is an invitation
/// to create an anchor rather than a failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocateReply {
    /// Whether the query resolved to a place at all.
    pub found: bool,
    /// Resolved latitude. Meaningless when `found` is false.
    pub lat: f64,
    /// Resolved longitude. Meaningless when `found` is false.
    pub lon: f64,
    /// Anchor whose patch contains the result, if any.
    pub anchor_id: Option<u32>,
    /// Block X of the result inside that patch, if any.
    pub mc_x: Option<i32>,
    /// Block Z of the result inside that patch, if any.
    pub mc_z: Option<i32>,
    /// Human-readable remark: the resolved display name, or why no patch matched.
    pub note: String,
}

/// Reply to [`RequestColumn`], carried on [`MSG_JSON_REPLY`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnReply {
    /// Echoed block X.
    pub x: i32,
    /// Echoed block Z.
    pub z: i32,
    /// Block Y of the highest non-air block in that column.
    pub surface_y: i32,
    /// True if the real surface lay outside the client's vertical range and `surface_y` is the
    /// clamped value.
    pub clipped: bool,
}

/// The body of a [`MSG_JSON_REPLY`] frame.
///
/// Serialised with an internal `kind` tag (`"elevationRange"`, `"anchor"`, `"locate"`,
/// `"column"`) so that a client can dispatch on the body alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JsonReply {
    /// `kind: "elevationRange"`.
    ElevationRange(ElevationRangeReply),
    /// `kind: "anchor"`.
    Anchor(AnchorReply),
    /// `kind: "locate"`.
    Locate(LocateReply),
    /// `kind: "column"`.
    Column(ColumnReply),
}

/// `Progress` (132): informational note sent zero or more times before a terminal message.
///
/// Stages are not guaranteed to occur, to occur in order, or to occur once. A cache-hit chunk
/// emits none at all, so clients must never use `Progress` for control flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// One of `"fetching_osm"`, `"fetching_elevation"`, `"generating"`, `"encoding"`.
    pub stage: String,
    /// Human-readable detail, e.g. `"tile 3,-7"`. Not machine-parsable.
    pub detail: String,
}

/// `Error` (133): terminal failure for one request. Never closes the connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorMessage {
    /// One of `"unknown_anchor"`, `"out_of_patch"`, `"generation_failed"`, `"cancelled"`,
    /// `"busy"`, `"bad_request"`, `"geocoding_unavailable"`.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

/// Any message the server can send.
///
/// `ChunkData` is much larger than the other variants, but it is also the one the server sends
/// in bulk, so boxing it would trade a cheap move for an allocation on the hot path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// `HelloOk` (128).
    HelloOk(HelloOk),
    /// `HelloError` (129).
    HelloError(HelloError),
    /// `ChunkData` (130). Encoded and DEFLATE-compressed by [`encode_server`]. A caller holding
    /// an already-encoded body (from a cache) can build the [`Frame`] directly instead.
    ChunkData(ChunkPayload),
    /// `ColumnData` (131), the generic JSON reply.
    JsonReply(JsonReply),
    /// `Progress` (132).
    Progress(Progress),
    /// `Error` (133).
    Error(ErrorMessage),
    /// `Pong` (134). Carries an empty JSON object.
    Pong,
}

/// Serialise a server message into a frame carrying `request_id`.
pub fn encode_server(msg: &ServerMessage, request_id: u64) -> Result<Frame, String> {
    /// Serialise a JSON body, naming the message in the error.
    fn json<T: Serialize>(name: &str, value: &T) -> Result<Vec<u8>, String> {
        serde_json::to_vec(value).map_err(|e| format!("failed to serialise {name}: {e}"))
    }

    let (msg_type, payload) = match msg {
        ServerMessage::HelloOk(m) => (MSG_HELLO_OK, json("HelloOk", m)?),
        ServerMessage::HelloError(m) => (MSG_HELLO_ERROR, json("HelloError", m)?),
        ServerMessage::ChunkData(chunk) => (MSG_CHUNK_DATA, encode_chunk(chunk)?),
        ServerMessage::JsonReply(m) => (MSG_JSON_REPLY, json("JSON reply", m)?),
        ServerMessage::Progress(m) => (MSG_PROGRESS, json("Progress", m)?),
        ServerMessage::Error(m) => (MSG_ERROR, json("Error", m)?),
        ServerMessage::Pong => (MSG_PONG, b"{}".to_vec()),
    };
    if payload.len() > MAX_PAYLOAD as usize {
        return Err(format!(
            "encoded message of {} bytes exceeds the {MAX_PAYLOAD} byte frame limit",
            payload.len()
        ));
    }
    Ok(Frame {
        msg_type,
        request_id,
        payload,
    })
}

// ---------------------------------------------------------------------------------------------
// Blockstate strings
// ---------------------------------------------------------------------------------------------

/// Render one block state as the vanilla blockstate string, e.g.
/// `minecraft:oak_stairs[facing=north,half=bottom]`.
///
/// `properties` is the NBT compound Arnis already keeps per cell (see
/// `world_editor::common::SectionToModify::properties`), a `Compound` of `String -> String` in
/// practice. Keys are sorted so the same state always renders to the same string, which lets the
/// client key its own lookup table on it and lets the palette deduplicate by string.
///
/// A property whose value is not a scalar (a list, a nested compound, an array) has no vanilla
/// blockstate spelling, so it is dropped rather than rendered as something unparsable. Numeric
/// and byte values are rendered as plain integers. A block with no properties, or a `properties`
/// value that is not a compound, renders as the bare name.
pub fn blockstate_string(name: &str, properties: Option<&fastnbt::Value>) -> String {
    let Some(fastnbt::Value::Compound(map)) = properties else {
        return name.to_string();
    };
    let mut pairs: Vec<(&str, String)> = map
        .iter()
        .filter_map(|(k, v)| property_value_string(v).map(|s| (k.as_str(), s)))
        .collect();
    if pairs.is_empty() {
        return name.to_string();
    }
    pairs.sort_by_key(|(key, _)| *key);

    let mut out = String::with_capacity(name.len() + pairs.len() * 16 + 2);
    out.push_str(name);
    out.push('[');
    for (i, (key, value)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(value);
    }
    out.push(']');
    out
}

/// Render one property value, or `None` for a value with no blockstate spelling.
fn property_value_string(value: &fastnbt::Value) -> Option<String> {
    match value {
        fastnbt::Value::String(s) => Some(s.clone()),
        fastnbt::Value::Byte(v) => Some(v.to_string()),
        fastnbt::Value::Short(v) => Some(v.to_string()),
        fastnbt::Value::Int(v) => Some(v.to_string()),
        fastnbt::Value::Long(v) => Some(v.to_string()),
        fastnbt::Value::Float(v) => Some(v.to_string()),
        fastnbt::Value::Double(v) => Some(v.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Data plane: chunk encoding
// ---------------------------------------------------------------------------------------------

/// One 16x16x16 section of a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionPayload {
    /// Entirely air. Nothing is written for it beyond its kind byte.
    Empty,
    /// One block state fills all 4096 cells.
    Uniform(String),
    /// A palette plus 4096 bit-packed indices in YZX order.
    Paletted {
        /// Distinct block states, as blockstate strings.
        palette: Vec<String>,
        /// One palette index per cell, `cell = (y & 15) * 256 + (z & 15) * 16 + (x & 15)`.
        indices: Vec<u16>,
    },
}

/// A block entity, described semantically rather than as Minecraft NBT.
///
/// The payload says what the object *is*; turning it into whatever the target engine stores is
/// the client's job, so every version-to-version NBT difference stays on the client side.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityPayload {
    /// Absolute block X.
    pub x: i32,
    /// Absolute block Y.
    pub y: i32,
    /// Absolute block Z.
    pub z: i32,
    /// Semantic kind: `"sign"`, `"chest"`, `"banner"`, `"bed"`, `"item_frame"`.
    pub kind: String,
    /// Small JSON object whose shape depends on `kind`, e.g.
    /// `{"lines":["a","b","c","d"],"color":"black","facing":2}`.
    pub data: serde_json::Value,
}

/// The decoded contents of a `ChunkData` (130) message.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkPayload {
    /// Chunk X, i.e. `block_x >> 4`.
    pub chunk_x: i32,
    /// Chunk Z, i.e. `block_z >> 4`.
    pub chunk_z: i32,
    /// Terrain in this chunk was clamped to the client's vertical range (flags bit 0).
    pub clipped: bool,
    /// Section index of the lowest section present; section `i` covers block Y `i*16..i*16+15`.
    pub min_section_y: i32,
    /// Sections low to high, contiguous. Sections outside the span are air.
    pub sections: Vec<SectionPayload>,
    /// Biome per cell of a 4x4 horizontal grid, `grid_index = z * 4 + x`, constant in Y.
    pub biomes: [String; 16],
    /// Block entities in this chunk.
    pub block_entities: Vec<BlockEntityPayload>,
}

/// Bits per palette index for a palette of `palette_len` entries.
///
/// `max(4, ceil(log2(palette_len)))` — four is the floor the Anvil format imposes. The wire
/// format caps a palette at `u16::MAX` entries, so the answer is never above 16; the loop still
/// stops at 32 so that no caller can drive `1usize << bits` past the width of `usize` and panic.
pub fn bits_per_index(palette_len: usize) -> u8 {
    let mut bits = 4u8;
    while bits < 32 && (1usize << bits) < palette_len {
        bits += 1;
    }
    bits
}

/// Pack indices into little-endian u64 words, `64 / bits` values per word.
///
/// Values never straddle a word boundary: the top `64 % bits` bits of each word are left as
/// zero padding. This is the Anvil 1.16+ rule, and it is what makes a client able to read a
/// single value with one shift and one mask.
fn pack_indices(indices: &[u16], bits: u8) -> Vec<u8> {
    let bits = usize::from(bits);
    let per_word = 64 / bits;
    let word_count = indices.len().div_ceil(per_word);
    let mut out = Vec::with_capacity(word_count * 8);
    for w in 0..word_count {
        let mut word: u64 = 0;
        for slot in 0..per_word {
            let i = w * per_word + slot;
            if i >= indices.len() {
                break;
            }
            word |= u64::from(indices[i]) << (slot * bits);
        }
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

/// Inverse of [`pack_indices`].
fn unpack_indices(data: &[u8], bits: u8, count: usize) -> Result<Vec<u16>, String> {
    if !(4..=16).contains(&bits) {
        return Err(format!("bits_per_index {bits} out of range 4..=16"));
    }
    let bits = usize::from(bits);
    let per_word = 64 / bits;
    let word_count = count.div_ceil(per_word);
    if data.len() != word_count * 8 {
        return Err(format!(
            "packed data is {} bytes, expected {} for {count} values at {bits} bits",
            data.len(),
            word_count * 8
        ));
    }
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(count);
    for word_bytes in data.chunks_exact(8) {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(word_bytes);
        let word = u64::from_le_bytes(buf);
        for slot in 0..per_word {
            if out.len() == count {
                break;
            }
            out.push(((word >> (slot * bits)) & mask) as u16);
        }
    }
    Ok(out)
}

/// Append a `[u16 len][utf8]` string.
fn write_str(out: &mut Vec<u8>, s: &str) -> Result<(), String> {
    let len = u16::try_from(s.len())
        .map_err(|_| format!("string of {} bytes exceeds the 65535 byte limit", s.len()))?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

/// Reject a length prefix that the bytes still in the buffer cannot possibly satisfy.
///
/// Every count and byte length in a chunk body arrives *before* the data it describes, so an
/// unchecked prefix lets a peer size an allocation with a handful of input bytes: a 22-byte
/// payload declaring `dataLen = u32::MAX` would reserve four gigabytes before the first read
/// fails. Checking the prefix against what is left turns that into an immediate `Err`.
///
/// `min_bytes_each` is the smallest number of bytes one element can occupy on the wire — 1 for a
/// raw byte, 2 for a `[u16 len][utf8]` string with an empty body, [`BLOCK_ENTITY_MIN_BYTES`] for
/// a block entity. The product uses `checked_mul` so no count can wrap past the buffer size.
fn check_available(
    remaining: usize,
    count: usize,
    min_bytes_each: usize,
    what: &str,
) -> Result<(), String> {
    let needed = count
        .checked_mul(min_bytes_each)
        .ok_or_else(|| format!("{what} of {count} overflows a usize"))?;
    if needed > remaining {
        return Err(format!(
            "{what} of {count} needs at least {needed} bytes but only {remaining} remain"
        ));
    }
    Ok(())
}

/// Read a `[u16 len][utf8]` string.
fn read_str(r: &mut &[u8]) -> Result<String, String> {
    let len = usize::from(read_u16(r)?);
    check_available(r.len(), len, 1, "string length")?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)
        .map_err(|_| "truncated string".to_string())?;
    String::from_utf8(buf).map_err(|e| format!("invalid UTF-8 in string: {e}"))
}

/// Read one little-endian u8.
fn read_u8(r: &mut &[u8]) -> Result<u8, String> {
    ReadBytesExt::read_u8(r).map_err(|_| "truncated chunk body".to_string())
}

/// Read one little-endian u16.
fn read_u16(r: &mut &[u8]) -> Result<u16, String> {
    ReadBytesExt::read_u16::<LittleEndian>(r).map_err(|_| "truncated chunk body".to_string())
}

/// Read one little-endian u32.
fn read_u32(r: &mut &[u8]) -> Result<u32, String> {
    ReadBytesExt::read_u32::<LittleEndian>(r).map_err(|_| "truncated chunk body".to_string())
}

/// Read one little-endian i32.
fn read_i32(r: &mut &[u8]) -> Result<i32, String> {
    ReadBytesExt::read_i32::<LittleEndian>(r).map_err(|_| "truncated chunk body".to_string())
}

/// Encode a chunk and DEFLATE-compress it, producing the `ChunkData` (130) payload.
///
/// Raw DEFLATE (RFC 1951): no zlib header, no gzip header, no trailing checksum. Paletted
/// encoding already removes the bulk of the redundancy; DEFLATE then collapses the long runs
/// that remain, which is why no further scheme is needed here.
pub fn encode_chunk(payload: &ChunkPayload) -> Result<Vec<u8>, String> {
    let mut raw: Vec<u8> = Vec::with_capacity(4096);

    let section_count = u16::try_from(payload.sections.len())
        .map_err(|_| format!("{} sections exceeds the u16 limit", payload.sections.len()))?;

    raw.extend_from_slice(&payload.chunk_x.to_le_bytes());
    raw.extend_from_slice(&payload.chunk_z.to_le_bytes());
    raw.push(u8::from(payload.clipped));
    raw.extend_from_slice(&payload.min_section_y.to_le_bytes());
    raw.extend_from_slice(&section_count.to_le_bytes());

    for (i, section) in payload.sections.iter().enumerate() {
        match section {
            SectionPayload::Empty => raw.push(0),
            SectionPayload::Uniform(name) => {
                raw.push(1);
                raw.extend_from_slice(&1u16.to_le_bytes());
                write_str(&mut raw, name)?;
            }
            SectionPayload::Paletted { palette, indices } => {
                if palette.is_empty() {
                    return Err(format!("section {i} has an empty palette"));
                }
                let palette_len = u16::try_from(palette.len()).map_err(|_| {
                    format!(
                        "section {i} palette of {} exceeds the u16 limit",
                        palette.len()
                    )
                })?;
                if indices.len() != SECTION_CELLS {
                    return Err(format!(
                        "section {i} has {} indices, expected {SECTION_CELLS}",
                        indices.len()
                    ));
                }
                if let Some(bad) = indices.iter().find(|&&ix| usize::from(ix) >= palette.len()) {
                    return Err(format!(
                        "section {i} index {bad} is out of range for a palette of {}",
                        palette.len()
                    ));
                }
                raw.push(2);
                raw.extend_from_slice(&palette_len.to_le_bytes());
                for entry in palette {
                    write_str(&mut raw, entry)?;
                }
                let bits = bits_per_index(palette.len());
                let data = pack_indices(indices, bits);
                raw.push(bits);
                let data_len = u32::try_from(data.len())
                    .map_err(|_| format!("section {i} packed data exceeds the u32 limit"))?;
                raw.extend_from_slice(&data_len.to_le_bytes());
                raw.extend_from_slice(&data);
            }
        }
    }

    // Biomes: a palette in first-appearance order plus one index per 4x4 grid cell.
    let mut biome_palette: Vec<&str> = Vec::new();
    let mut biome_indices = [0u8; 16];
    for (cell, biome) in payload.biomes.iter().enumerate() {
        let index = match biome_palette.iter().position(|b| *b == biome.as_str()) {
            Some(index) => index,
            None => {
                biome_palette.push(biome.as_str());
                biome_palette.len() - 1
            }
        };
        biome_indices[cell] = index as u8;
    }
    raw.push(biome_palette.len() as u8);
    for entry in &biome_palette {
        write_str(&mut raw, entry)?;
    }
    raw.extend_from_slice(&biome_indices);

    // Block entities.
    let be_count = u16::try_from(payload.block_entities.len()).map_err(|_| {
        format!(
            "{} block entities exceeds the u16 limit",
            payload.block_entities.len()
        )
    })?;
    raw.extend_from_slice(&be_count.to_le_bytes());
    for entity in &payload.block_entities {
        raw.extend_from_slice(&entity.x.to_le_bytes());
        raw.extend_from_slice(&entity.y.to_le_bytes());
        raw.extend_from_slice(&entity.z.to_le_bytes());
        write_str(&mut raw, &entity.kind)?;
        let json = serde_json::to_string(&entity.data)
            .map_err(|e| format!("failed to serialise block entity data: {e}"))?;
        write_str(&mut raw, &json)?;
    }

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&raw)
        .map_err(|e| format!("failed to compress chunk: {e}"))?;
    encoder
        .finish()
        .map_err(|e| format!("failed to finish chunk compression: {e}"))
}

/// Decode a `ChunkData` (130) payload.
///
/// Provided for tests and for third-party clients written in Rust; the server itself only
/// encodes. Decompression is bounded by [`MAX_CHUNK_RAW`] so a small hostile frame cannot
/// inflate into an out-of-memory abort, and every length prefix inside the body — section count,
/// palette length, string length, packed data length, biome palette length, block entity count —
/// is checked against the bytes actually remaining by [`check_available`] *before* anything is
/// reserved or allocated for it. Malformed input always returns `Err`; it never panics.
pub fn decode_chunk(bytes: &[u8]) -> Result<ChunkPayload, String> {
    let mut raw = Vec::new();
    DeflateDecoder::new(bytes)
        .take(MAX_CHUNK_RAW + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("failed to decompress chunk: {e}"))?;
    if raw.len() as u64 > MAX_CHUNK_RAW {
        return Err(format!(
            "decompressed chunk exceeds the {MAX_CHUNK_RAW} byte limit"
        ));
    }

    let mut r: &[u8] = &raw;
    let chunk_x = read_i32(&mut r)?;
    let chunk_z = read_i32(&mut r)?;
    let flags = read_u8(&mut r)?;
    let clipped = flags & 1 != 0;
    let min_section_y = read_i32(&mut r)?;
    let section_count = usize::from(read_u16(&mut r)?);
    // A section is at least its one-byte kind, so a count above the bytes left is a framing lie.
    check_available(r.len(), section_count, 1, "section count")?;

    let mut sections = Vec::with_capacity(section_count);
    for i in 0..section_count {
        let kind = read_u8(&mut r)?;
        match kind {
            0 => sections.push(SectionPayload::Empty),
            1 => {
                let palette_len = read_u16(&mut r)?;
                if palette_len != 1 {
                    return Err(format!(
                        "section {i} is uniform but declares a palette of {palette_len}"
                    ));
                }
                sections.push(SectionPayload::Uniform(read_str(&mut r)?));
            }
            2 => {
                let palette_len = usize::from(read_u16(&mut r)?);
                if palette_len == 0 {
                    return Err(format!("section {i} has an empty palette"));
                }
                // Every palette entry is at least its two-byte length prefix.
                check_available(r.len(), palette_len, 2, "palette length")
                    .map_err(|e| format!("section {i} {e}"))?;
                let mut palette = Vec::with_capacity(palette_len);
                for _ in 0..palette_len {
                    palette.push(read_str(&mut r)?);
                }
                let bits = read_u8(&mut r)?;
                if bits != bits_per_index(palette_len) {
                    return Err(format!(
                        "section {i} declares {bits} bits per index, expected {} for a palette \
                         of {palette_len}",
                        bits_per_index(palette_len)
                    ));
                }
                let data_len = usize::try_from(read_u32(&mut r)?)
                    .map_err(|_| format!("section {i} packed data length exceeds a usize"))?;
                check_available(r.len(), data_len, 1, "packed data length")
                    .map_err(|e| format!("section {i} {e}"))?;
                let mut data = vec![0u8; data_len];
                r.read_exact(&mut data)
                    .map_err(|_| format!("section {i} packed data is truncated"))?;
                let indices = unpack_indices(&data, bits, SECTION_CELLS)?;
                if let Some(bad) = indices.iter().find(|&&ix| usize::from(ix) >= palette_len) {
                    return Err(format!(
                        "section {i} index {bad} is out of range for a palette of {palette_len}"
                    ));
                }
                sections.push(SectionPayload::Paletted { palette, indices });
            }
            other => return Err(format!("section {i} has unknown kind byte {other}")),
        }
    }

    let biome_palette_len = usize::from(read_u8(&mut r)?);
    if biome_palette_len == 0 {
        return Err("biome palette is empty".to_string());
    }
    check_available(r.len(), biome_palette_len, 2, "biome palette length")?;
    let mut biome_palette = Vec::with_capacity(biome_palette_len);
    for _ in 0..biome_palette_len {
        biome_palette.push(read_str(&mut r)?);
    }
    let mut biome_index_bytes = [0u8; 16];
    r.read_exact(&mut biome_index_bytes)
        .map_err(|_| "truncated biome indices".to_string())?;
    let mut biome_vec = Vec::with_capacity(16);
    for index in biome_index_bytes {
        let entry = biome_palette
            .get(usize::from(index))
            .ok_or_else(|| format!("biome index {index} is out of range"))?;
        biome_vec.push(entry.clone());
    }
    let biomes: [String; 16] = biome_vec
        .try_into()
        .map_err(|_| "biome grid is not 16 cells".to_string())?;

    let be_count = usize::from(read_u16(&mut r)?);
    check_available(
        r.len(),
        be_count,
        BLOCK_ENTITY_MIN_BYTES,
        "block entity count",
    )?;
    let mut block_entities = Vec::with_capacity(be_count);
    for _ in 0..be_count {
        let x = read_i32(&mut r)?;
        let y = read_i32(&mut r)?;
        let z = read_i32(&mut r)?;
        let kind = read_str(&mut r)?;
        let json = read_str(&mut r)?;
        let data = serde_json::from_str(&json)
            .map_err(|e| format!("invalid block entity JSON for {kind}: {e}"))?;
        block_entities.push(BlockEntityPayload {
            x,
            y,
            z,
            kind,
            data,
        });
    }

    if !r.is_empty() {
        return Err(format!("{} trailing bytes after chunk body", r.len()));
    }

    Ok(ChunkPayload {
        chunk_x,
        chunk_z,
        clipped,
        min_section_y,
        sections,
        biomes,
        block_entities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Cursor;

    // -- framing ------------------------------------------------------------------------------

    /// Write a frame and read it straight back out of the bytes.
    fn round_trip(frame: &Frame) -> Frame {
        let mut buf = Vec::new();
        write_frame(&mut buf, frame).expect("write");
        assert_eq!(
            buf.len(),
            frame.payload.len() + 13,
            "frame size = payload + 13"
        );
        read_frame(&mut Cursor::new(buf)).expect("read")
    }

    #[test]
    fn frame_round_trips_for_every_message_type() {
        let types = [
            MSG_HELLO,
            MSG_QUERY_ELEVATION_RANGE,
            MSG_ADD_ANCHOR,
            MSG_REQUEST_CHUNK,
            MSG_REQUEST_COLUMN,
            MSG_LOCATE,
            MSG_PREFETCH,
            MSG_CANCEL,
            MSG_PING,
            MSG_HELLO_OK,
            MSG_HELLO_ERROR,
            MSG_CHUNK_DATA,
            MSG_JSON_REPLY,
            MSG_PROGRESS,
            MSG_ERROR,
            MSG_PONG,
        ];
        for (i, msg_type) in types.iter().enumerate() {
            let frame = Frame {
                msg_type: *msg_type,
                request_id: (i as u64) * 1_000_003 + 7,
                payload: vec![*msg_type; i + 1],
            };
            assert_eq!(round_trip(&frame), frame, "type {msg_type}");
        }
    }

    #[test]
    fn frame_round_trips_an_empty_payload() {
        let frame = Frame {
            msg_type: MSG_PONG,
            request_id: u64::MAX,
            payload: Vec::new(),
        };
        assert_eq!(round_trip(&frame), frame);
    }

    #[test]
    fn frames_are_read_back_to_back_from_one_stream() {
        let a = Frame {
            msg_type: MSG_PING,
            request_id: 1,
            payload: b"{}".to_vec(),
        };
        let b = Frame {
            msg_type: MSG_PROGRESS,
            request_id: 2,
            payload: b"{\"stage\":\"generating\",\"detail\":\"tile 0,0\"}".to_vec(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).unwrap();
        write_frame(&mut buf, &b).unwrap();
        let mut cursor = Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).unwrap(), a);
        assert_eq!(read_frame(&mut cursor).unwrap(), b);
        assert_eq!(
            read_frame(&mut cursor).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof,
            "the stream is exhausted after both frames"
        );
    }

    #[test]
    fn header_is_exactly_thirteen_little_endian_bytes() {
        // The worked example from docs/stream-protocol.md: Ping, request_id 7, payload "{}".
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &Frame {
                msg_type: MSG_PING,
                request_id: 7,
                payload: b"{}".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(
            buf,
            vec![0x02, 0x00, 0x00, 0x00, 0x09, 0x07, 0, 0, 0, 0, 0, 0, 0, 0x7B, 0x7D]
        );
    }

    #[test]
    fn frame_at_the_size_limit_is_accepted() {
        let mut header = Vec::new();
        header.extend_from_slice(&MAX_PAYLOAD.to_le_bytes());
        header.push(MSG_CHUNK_DATA);
        header.extend_from_slice(&99u64.to_le_bytes());
        // The payload is generated rather than materialised twice, so the test holds one copy.
        let mut stream = Cursor::new(header).chain(io::repeat(0xAB).take(u64::from(MAX_PAYLOAD)));
        let frame = read_frame(&mut stream).expect("a frame exactly at the limit is legal");
        assert_eq!(frame.msg_type, MSG_CHUNK_DATA);
        assert_eq!(frame.request_id, 99);
        assert_eq!(frame.payload.len(), MAX_PAYLOAD as usize);
        assert!(frame.payload.iter().all(|b| *b == 0xAB));
    }

    #[test]
    fn frame_above_the_size_limit_is_rejected_without_allocating() {
        for declared in [MAX_PAYLOAD + 1, u32::MAX] {
            // Only the 4-byte length prefix is present: if `read_frame` allocated or tried to
            // read the payload it would fail with something other than InvalidData.
            let bytes = declared.to_le_bytes().to_vec();
            let err = read_frame(&mut Cursor::new(bytes)).unwrap_err();
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidData,
                "declared {declared}"
            );
        }
    }

    #[test]
    fn truncated_frames_report_unexpected_eof() {
        // Header cut short.
        assert_eq!(
            read_frame(&mut Cursor::new(vec![0x02, 0x00, 0x00]))
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        // Header complete, payload missing entirely.
        let mut header = Vec::new();
        header.extend_from_slice(&16u32.to_le_bytes());
        header.push(MSG_PING);
        header.extend_from_slice(&3u64.to_le_bytes());
        assert_eq!(
            read_frame(&mut Cursor::new(header)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        // Payload cut short by one byte.
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &Frame {
                msg_type: MSG_ERROR,
                request_id: 4,
                payload: b"{\"code\":\"busy\",\"message\":\"\"}".to_vec(),
            },
        )
        .unwrap();
        buf.pop();
        assert_eq!(
            read_frame(&mut Cursor::new(buf)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        // Nothing at all.
        assert_eq!(
            read_frame(&mut Cursor::new(Vec::new())).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    // -- control plane ------------------------------------------------------------------------

    fn sample_vertical() -> VerticalMapping {
        VerticalMapping {
            min_y: -64,
            height: 384,
            sea_level_y: 62,
            vertical_scale: 1.0,
        }
    }

    fn sample_config() -> GenConfig {
        GenConfig {
            scale: 1.0,
            fillground: true,
            interior: false,
            use3d: true,
            overture: false,
            canopy_height: true,
            terrain_only: false,
            flat_ground: false,
            local_osm_file: None,
        }
    }

    #[test]
    fn hello_decodes_with_camel_case_fields() {
        let json = br#"{
            "protocolVersion": 1,
            "clientName": "arnis-bridge",
            "clientVersion": "0.1.0",
            "sessionToken": "deadbeef",
            "config": {
                "scale": 1.0, "fillground": true, "interior": false, "use3d": true,
                "overture": false, "canopyHeight": true, "terrainOnly": false,
                "localOsmFile": "/tmp/area.osm"
            },
            "vertical": { "minY": -64, "height": 384, "seaLevelY": 62, "verticalScale": 1.0 },
            "anchors": [
                { "id": 1, "lat": 54.63, "lon": 9.93, "mcX": 0, "mcZ": 0, "radiusM": 2000.0 }
            ]
        }"#;
        let frame = Frame {
            msg_type: MSG_HELLO,
            request_id: 1,
            payload: json.to_vec(),
        };
        let ClientMessage::Hello(hello) = decode_client(&frame).unwrap() else {
            panic!("expected Hello");
        };
        assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
        assert_eq!(hello.client_name, "arnis-bridge");
        assert_eq!(
            hello.config.local_osm_file.as_deref(),
            Some("/tmp/area.osm")
        );
        assert!(hello.config.canopy_height);
        assert_eq!(hello.vertical, sample_vertical());
        assert_eq!(hello.anchors.len(), 1);
        assert_eq!(hello.anchors[0].mc_x, 0);
        assert_eq!(hello.anchors[0].radius_m, 2000.0);
    }

    #[test]
    fn client_messages_round_trip_through_frames() {
        let cases: Vec<(u8, serde_json::Value, ClientMessage)> = vec![
            (
                MSG_HELLO,
                serde_json::to_value(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "c".into(),
                    client_version: "v".into(),
                    session_token: "t".into(),
                    config: sample_config(),
                    vertical: sample_vertical(),
                    anchors: vec![],
                })
                .unwrap(),
                ClientMessage::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "c".into(),
                    client_version: "v".into(),
                    session_token: "t".into(),
                    config: sample_config(),
                    vertical: sample_vertical(),
                    anchors: vec![],
                }),
            ),
            (
                MSG_QUERY_ELEVATION_RANGE,
                serde_json::json!({"lat": 1.5, "lon": -2.5, "radiusM": 300.0}),
                ClientMessage::QueryElevationRange(QueryElevationRange {
                    lat: 1.5,
                    lon: -2.5,
                    radius_m: 300.0,
                }),
            ),
            (
                MSG_ADD_ANCHOR,
                serde_json::json!({"lat": 1.0, "lon": 2.0}),
                ClientMessage::AddAnchor(AddAnchor {
                    id: None,
                    lat: 1.0,
                    lon: 2.0,
                    radius_m: None,
                }),
            ),
            (
                MSG_REQUEST_CHUNK,
                serde_json::json!({"chunkX": -3, "chunkZ": 9, "anchorId": 2}),
                ClientMessage::RequestChunk(RequestChunk {
                    chunk_x: -3,
                    chunk_z: 9,
                    anchor_id: Some(2),
                }),
            ),
            (
                MSG_REQUEST_COLUMN,
                serde_json::json!({"x": 10, "z": -10}),
                ClientMessage::RequestColumn(RequestColumn {
                    x: 10,
                    z: -10,
                    anchor_id: None,
                }),
            ),
            (
                MSG_LOCATE,
                serde_json::json!({"query": "Arnis, Germany"}),
                ClientMessage::Locate(Locate {
                    query: "Arnis, Germany".into(),
                }),
            ),
            (
                MSG_PREFETCH,
                serde_json::json!({"chunkX": 0, "chunkZ": 0, "radiusChunks": 6}),
                ClientMessage::Prefetch(Prefetch {
                    chunk_x: 0,
                    chunk_z: 0,
                    radius_chunks: Some(6),
                    anchor_id: None,
                }),
            ),
            (
                MSG_CANCEL,
                serde_json::json!({"requestId": 77}),
                ClientMessage::Cancel(Cancel {
                    request_id: Some(77),
                }),
            ),
            (MSG_PING, serde_json::json!({}), ClientMessage::Ping),
        ];

        for (msg_type, body, expected) in cases {
            let frame = Frame {
                msg_type,
                request_id: 12,
                payload: serde_json::to_vec(&body).unwrap(),
            };
            let decoded = decode_client(&round_trip(&frame)).unwrap();
            assert_eq!(decoded, expected, "type {msg_type}");
        }
    }

    #[test]
    fn decode_client_rejects_unknown_types_and_bad_json() {
        let unknown = Frame {
            msg_type: 200,
            request_id: 0,
            payload: b"{}".to_vec(),
        };
        assert!(decode_client(&unknown)
            .unwrap_err()
            .contains("unknown message type"));

        let malformed = Frame {
            msg_type: MSG_LOCATE,
            request_id: 0,
            payload: b"{".to_vec(),
        };
        assert!(decode_client(&malformed).unwrap_err().contains("Locate"));

        let missing_field = Frame {
            msg_type: MSG_REQUEST_CHUNK,
            request_id: 0,
            payload: b"{\"chunkX\":1}".to_vec(),
        };
        assert!(decode_client(&missing_field).is_err());
    }

    #[test]
    fn server_messages_encode_on_the_right_type_bytes() {
        let cases = vec![
            (
                ServerMessage::HelloOk(HelloOk {
                    arnis_version: "3.1.0".into(),
                    protocol_version: PROTOCOL_VERSION,
                    tile_size: 512,
                    max_in_flight: 4,
                    capabilities: vec!["prefetch".into()],
                }),
                MSG_HELLO_OK,
            ),
            (
                ServerMessage::HelloError(HelloError {
                    reason: "nope".into(),
                    code: "bad_token".into(),
                }),
                MSG_HELLO_ERROR,
            ),
            (
                ServerMessage::Progress(Progress {
                    stage: "generating".into(),
                    detail: "tile 1,1".into(),
                }),
                MSG_PROGRESS,
            ),
            (
                ServerMessage::Error(ErrorMessage {
                    code: "busy".into(),
                    message: "too many requests in flight".into(),
                }),
                MSG_ERROR,
            ),
            (ServerMessage::Pong, MSG_PONG),
        ];
        for (msg, expected_type) in cases {
            let frame = encode_server(&msg, 5).unwrap();
            assert_eq!(frame.msg_type, expected_type);
            assert_eq!(frame.request_id, 5);
            // Every control-plane payload is a JSON object.
            let value: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap();
            assert!(value.is_object());
        }
        assert_eq!(
            encode_server(&ServerMessage::Pong, 1).unwrap().payload,
            b"{}"
        );
    }

    #[test]
    fn hello_ok_uses_camel_case_on_the_wire() {
        let frame = encode_server(
            &ServerMessage::HelloOk(HelloOk {
                arnis_version: "3.1.0".into(),
                protocol_version: 1,
                tile_size: 512,
                max_in_flight: 4,
                capabilities: vec![],
            }),
            1,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(value["arnisVersion"], "3.1.0");
        assert_eq!(value["tileSize"], 512);
        assert_eq!(value["maxInFlight"], 4);
    }

    #[test]
    fn json_replies_carry_their_kind_discriminator() {
        let replies = vec![
            (
                JsonReply::ElevationRange(ElevationRangeReply {
                    min_elevation_m: -3.0,
                    max_elevation_m: 120.0,
                    recommended_min_y: -64,
                    recommended_height: 384,
                    recommended_sea_level_y: 62,
                }),
                "elevationRange",
            ),
            (
                JsonReply::Anchor(AnchorReply {
                    id: 3,
                    lat: 1.0,
                    lon: 2.0,
                    mc_x: 100,
                    mc_z: -100,
                    radius_m: 5000.0,
                }),
                "anchor",
            ),
            (
                JsonReply::Locate(LocateReply {
                    found: true,
                    lat: 1.0,
                    lon: 2.0,
                    anchor_id: Some(3),
                    mc_x: Some(1),
                    mc_z: Some(2),
                    note: "Arnis".into(),
                }),
                "locate",
            ),
            (
                JsonReply::Column(ColumnReply {
                    x: 4,
                    z: 5,
                    surface_y: 71,
                    clipped: false,
                }),
                "column",
            ),
        ];
        for (reply, kind) in replies {
            let frame = encode_server(&ServerMessage::JsonReply(reply.clone()), 8).unwrap();
            assert_eq!(frame.msg_type, MSG_JSON_REPLY);
            let value: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap();
            assert_eq!(value["kind"], kind);
            let parsed: JsonReply = serde_json::from_slice(&frame.payload).unwrap();
            assert_eq!(parsed, reply);
        }
    }

    // -- vertical mapping ---------------------------------------------------------------------

    #[test]
    fn vertical_mapping_accepts_a_realistic_world() {
        assert!(sample_vertical().validate().is_ok());
        // A world sized for alpine terrain: sea level pushed well below zero.
        assert!(VerticalMapping {
            min_y: -2032,
            height: 4064,
            sea_level_y: -1900,
            vertical_scale: 1.0,
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn vertical_mapping_rejects_each_constraint_individually() {
        let bad_min_y = VerticalMapping {
            min_y: -60,
            ..sample_vertical()
        };
        assert!(bad_min_y.validate().unwrap_err().contains("minY"));

        let bad_height = VerticalMapping {
            height: 300,
            ..sample_vertical()
        };
        assert!(bad_height.validate().unwrap_err().contains("height"));

        let too_tall = VerticalMapping {
            min_y: -2048,
            height: 4080,
            sea_level_y: 0,
            vertical_scale: 1.0,
        };
        assert!(too_tall.validate().unwrap_err().contains("4064"));

        let too_high = VerticalMapping {
            min_y: 0,
            height: 4064,
            sea_level_y: 62,
            vertical_scale: 1.0,
        };
        assert!(too_high.validate().unwrap_err().contains("2032"));

        for sea in [-64, -128, 320, 1000] {
            let bad_sea = VerticalMapping {
                sea_level_y: sea,
                ..sample_vertical()
            };
            assert!(
                bad_sea.validate().unwrap_err().contains("seaLevelY"),
                "seaLevelY {sea} must be rejected"
            );
        }

        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let bad_scale = VerticalMapping {
                vertical_scale: scale,
                ..sample_vertical()
            };
            assert!(
                bad_scale.validate().unwrap_err().contains("verticalScale"),
                "verticalScale {scale} must be rejected"
            );
        }

        let non_positive = VerticalMapping {
            height: 0,
            ..sample_vertical()
        };
        assert!(non_positive.validate().is_err());
    }

    #[test]
    fn vertical_mapping_rejects_a_floor_below_the_engine_limit() {
        // Why the bound has to exist: the Anvil section index is `y >> 4` stored in an i8, and a
        // floor of -4048 puts it at -253, which wraps to 3. `world_section_range()` then reports
        // a minimum above its maximum and blocks are filed into completely wrong sections.
        assert_eq!(-4048i32 >> 4, -253);
        assert_eq!(-253i32 as i8, 3);
        // -2032 is exactly where it stops being safe: -2048 >> 4 is -128, the i8 floor, and one
        // section deeper is -129, which wraps to 127.
        assert_eq!(-2048i32 >> 4, -128);
        assert_eq!(-2064i32 >> 4, -129);
        assert_eq!(-129i32 as i8, 127);

        // Each of these satisfies every other constraint: 16-aligned, height <= 4064,
        // minY + height <= 2032, and seaLevelY strictly inside the world.
        let below_floor = [
            VerticalMapping {
                min_y: -4048,
                height: 4064,
                sea_level_y: 0,
                vertical_scale: 1.0,
            },
            VerticalMapping {
                min_y: -4096,
                height: 4064,
                sea_level_y: -4000,
                vertical_scale: 1.0,
            },
            // The shallowest floor that wraps: one section below the i8 section floor.
            VerticalMapping {
                min_y: -2064,
                height: 4064,
                sea_level_y: -2050,
                vertical_scale: 1.0,
            },
            VerticalMapping {
                min_y: i32::MIN,
                height: 16,
                sea_level_y: i32::MIN + 8,
                vertical_scale: 1.0,
            },
        ];
        for mapping in &below_floor {
            let err = mapping
                .validate()
                .expect_err("a floor below the engine limit must be rejected");
            assert!(
                err.contains("minY") && err.contains("-2032"),
                "minY {} must be rejected on the floor bound, got: {err}",
                mapping.min_y
            );
        }

        // The floor itself, and an ordinary vanilla world, still validate.
        assert!(VerticalMapping {
            min_y: ENGINE_FLOOR,
            height: 4064,
            sea_level_y: -1900,
            vertical_scale: 1.0,
        }
        .validate()
        .is_ok());
        assert!(VerticalMapping {
            min_y: -64,
            height: 384,
            sea_level_y: 62,
            vertical_scale: 1.0,
        }
        .validate()
        .is_ok());

        // A world that is both too deep and too tall still reports the height first, so the
        // existing `too_tall` expectation keeps holding.
        assert!(VerticalMapping {
            min_y: -2048,
            height: 4080,
            sea_level_y: 0,
            vertical_scale: 1.0,
        }
        .validate()
        .unwrap_err()
        .contains("4064"));
    }

    // -- blockstate strings -------------------------------------------------------------------

    /// Build the `String -> String` compound Arnis stores per cell.
    fn props(pairs: &[(&str, &str)]) -> fastnbt::Value {
        let mut map: HashMap<String, fastnbt::Value> = HashMap::new();
        for (k, v) in pairs {
            map.insert((*k).to_string(), fastnbt::Value::String((*v).to_string()));
        }
        fastnbt::Value::Compound(map)
    }

    #[test]
    fn blockstate_string_without_properties_is_the_bare_name() {
        assert_eq!(
            blockstate_string("minecraft:stone", None),
            "minecraft:stone"
        );
        let empty = fastnbt::Value::Compound(HashMap::new());
        assert_eq!(
            blockstate_string("minecraft:stone", Some(&empty)),
            "minecraft:stone"
        );
        // A non-compound properties value has no blockstate spelling at all.
        let odd = fastnbt::Value::String("facing=north".to_string());
        assert_eq!(
            blockstate_string("minecraft:stone", Some(&odd)),
            "minecraft:stone"
        );
    }

    #[test]
    fn blockstate_string_with_one_property() {
        let p = props(&[("persistent", "true")]);
        assert_eq!(
            blockstate_string("minecraft:oak_leaves", Some(&p)),
            "minecraft:oak_leaves[persistent=true]"
        );
    }

    #[test]
    fn blockstate_string_sorts_properties_by_key() {
        let p = props(&[
            ("waterlogged", "false"),
            ("half", "bottom"),
            ("facing", "north"),
            ("shape", "straight"),
        ]);
        assert_eq!(
            blockstate_string("minecraft:oak_stairs", Some(&p)),
            "minecraft:oak_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]"
        );
    }

    #[test]
    fn blockstate_string_handles_non_string_property_values() {
        let mut map: HashMap<String, fastnbt::Value> = HashMap::new();
        map.insert("age".to_string(), fastnbt::Value::Int(3));
        map.insert("level".to_string(), fastnbt::Value::Byte(7));
        map.insert("facing".to_string(), fastnbt::Value::String("east".into()));
        // A structured value has no blockstate spelling and is dropped rather than panicking.
        map.insert(
            "junk".to_string(),
            fastnbt::Value::List(vec![fastnbt::Value::Int(1)]),
        );
        let value = fastnbt::Value::Compound(map);
        assert_eq!(
            blockstate_string("minecraft:wheat", Some(&value)),
            "minecraft:wheat[age=3,facing=east,level=7]"
        );

        // Every value structured: the name survives on its own.
        let mut only_junk: HashMap<String, fastnbt::Value> = HashMap::new();
        only_junk.insert("junk".to_string(), fastnbt::Value::List(vec![]));
        assert_eq!(
            blockstate_string(
                "minecraft:stone",
                Some(&fastnbt::Value::Compound(only_junk))
            ),
            "minecraft:stone"
        );
    }

    // -- chunk encoding -----------------------------------------------------------------------

    /// A uniform 4x4 biome grid.
    fn biome_grid(name: &str) -> [String; 16] {
        std::array::from_fn(|_| name.to_string())
    }

    fn chunk_round_trip(payload: &ChunkPayload) -> ChunkPayload {
        let bytes = encode_chunk(payload).expect("encode");
        decode_chunk(&bytes).expect("decode")
    }

    #[test]
    fn all_air_chunk_round_trips() {
        let payload = ChunkPayload {
            chunk_x: -7,
            chunk_z: 13,
            clipped: false,
            min_section_y: -4,
            sections: vec![SectionPayload::Empty; 24],
            biomes: biome_grid("minecraft:plains"),
            block_entities: vec![],
        };
        let decoded = chunk_round_trip(&payload);
        assert_eq!(decoded, payload);
        assert!(decoded
            .sections
            .iter()
            .all(|s| matches!(s, SectionPayload::Empty)));
    }

    #[test]
    fn uniform_stone_chunk_round_trips() {
        let payload = ChunkPayload {
            chunk_x: 0,
            chunk_z: 0,
            clipped: true,
            min_section_y: 0,
            sections: vec![
                SectionPayload::Uniform("minecraft:stone".to_string()),
                SectionPayload::Uniform("minecraft:oak_leaves[persistent=true]".to_string()),
                SectionPayload::Empty,
            ],
            biomes: biome_grid("minecraft:forest"),
            block_entities: vec![],
        };
        let decoded = chunk_round_trip(&payload);
        assert_eq!(decoded, payload);
        assert!(decoded.clipped, "the clipped flag survives the round trip");
    }

    #[test]
    fn mixed_biome_grid_round_trips() {
        let mut biomes = biome_grid("minecraft:plains");
        biomes[0] = "minecraft:river".to_string();
        biomes[5] = "minecraft:beach".to_string();
        biomes[15] = "minecraft:river".to_string();
        let payload = ChunkPayload {
            chunk_x: 1,
            chunk_z: 2,
            clipped: false,
            min_section_y: -4,
            sections: vec![SectionPayload::Empty],
            biomes,
            block_entities: vec![],
        };
        assert_eq!(chunk_round_trip(&payload), payload);
    }

    #[test]
    fn large_palette_chunk_round_trips_at_nine_bits() {
        let palette: Vec<String> = (0..300).map(|i| format!("minecraft:block_{i}")).collect();
        assert_eq!(bits_per_index(palette.len()), 9, "300 entries need 9 bits");
        let indices: Vec<u16> = (0..SECTION_CELLS).map(|i| (i % 300) as u16).collect();
        let payload = ChunkPayload {
            chunk_x: 42,
            chunk_z: -42,
            clipped: false,
            min_section_y: -4,
            sections: vec![
                SectionPayload::Empty,
                SectionPayload::Paletted {
                    palette: palette.clone(),
                    indices: indices.clone(),
                },
            ],
            biomes: biome_grid("minecraft:plains"),
            block_entities: vec![],
        };
        let decoded = chunk_round_trip(&payload);
        assert_eq!(decoded, payload);
        let SectionPayload::Paletted {
            palette: got_palette,
            indices: got_indices,
        } = &decoded.sections[1]
        else {
            panic!("expected a paletted section");
        };
        assert_eq!(got_palette.len(), 300);
        assert_eq!(got_indices.len(), SECTION_CELLS);
        assert_eq!(got_indices[299], 299);
        assert_eq!(got_indices[300], 0);
    }

    #[test]
    fn small_palette_chunk_round_trips_at_the_four_bit_floor() {
        let palette = vec![
            "minecraft:air".to_string(),
            "minecraft:stone".to_string(),
            "minecraft:oak_stairs[facing=north,half=bottom]".to_string(),
        ];
        assert_eq!(bits_per_index(palette.len()), 4, "the floor is 4 bits");
        let indices: Vec<u16> = (0..SECTION_CELLS).map(|i| (i % 3) as u16).collect();
        let payload = ChunkPayload {
            chunk_x: 3,
            chunk_z: 3,
            clipped: false,
            min_section_y: 0,
            sections: vec![SectionPayload::Paletted { palette, indices }],
            biomes: biome_grid("minecraft:plains"),
            block_entities: vec![],
        };
        assert_eq!(chunk_round_trip(&payload), payload);
    }

    #[test]
    fn chunk_with_block_entities_round_trips() {
        let payload = ChunkPayload {
            chunk_x: -1,
            chunk_z: -1,
            clipped: false,
            min_section_y: -4,
            sections: vec![SectionPayload::Uniform("minecraft:stone".to_string())],
            biomes: biome_grid("minecraft:plains"),
            block_entities: vec![
                BlockEntityPayload {
                    x: -16,
                    y: 71,
                    z: -3,
                    kind: "sign".to_string(),
                    data: serde_json::json!({
                        "lines": ["Hauptstraße", "", "", ""],
                        "color": "black",
                        "facing": 2
                    }),
                },
                BlockEntityPayload {
                    x: -10,
                    y: 70,
                    z: -10,
                    kind: "chest".to_string(),
                    data: serde_json::json!({"facing": "north"}),
                },
                BlockEntityPayload {
                    x: -1,
                    y: 64,
                    z: -1,
                    kind: "item_frame".to_string(),
                    data: serde_json::json!({}),
                },
            ],
        };
        let decoded = chunk_round_trip(&payload);
        assert_eq!(decoded, payload);
        assert_eq!(decoded.block_entities[0].data["lines"][0], "Hauptstraße");
    }

    #[test]
    fn encode_chunk_rejects_malformed_sections() {
        let base = ChunkPayload {
            chunk_x: 0,
            chunk_z: 0,
            clipped: false,
            min_section_y: 0,
            sections: vec![],
            biomes: biome_grid("minecraft:plains"),
            block_entities: vec![],
        };

        let empty_palette = ChunkPayload {
            sections: vec![SectionPayload::Paletted {
                palette: vec![],
                indices: vec![0; SECTION_CELLS],
            }],
            ..base.clone()
        };
        assert!(encode_chunk(&empty_palette)
            .unwrap_err()
            .contains("empty palette"));

        let short = ChunkPayload {
            sections: vec![SectionPayload::Paletted {
                palette: vec!["minecraft:stone".into()],
                indices: vec![0; 10],
            }],
            ..base.clone()
        };
        assert!(encode_chunk(&short).unwrap_err().contains("10 indices"));

        let out_of_range = ChunkPayload {
            sections: vec![SectionPayload::Paletted {
                palette: vec!["minecraft:stone".into()],
                indices: vec![5; SECTION_CELLS],
            }],
            ..base
        };
        assert!(encode_chunk(&out_of_range)
            .unwrap_err()
            .contains("out of range"));
    }

    #[test]
    fn decode_chunk_rejects_garbage() {
        assert!(decode_chunk(&[0xFF, 0x00, 0x13, 0x37]).is_err());
        // Valid DEFLATE, truncated body.
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[1, 2, 3]).unwrap();
        let bytes = encoder.finish().unwrap();
        assert!(decode_chunk(&bytes).is_err());
    }

    // -- adversarial chunk bodies -------------------------------------------------------------

    /// DEFLATE a raw chunk body the way `encode_chunk` does, so `decode_chunk` reaches the parser.
    fn deflate(raw: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(raw).expect("deflate");
        encoder.finish().expect("finish")
    }

    /// The fixed 15-byte prologue: `[i32 x][i32 z][u8 flags][i32 minSectionY][u16 sectionCount]`.
    fn chunk_prologue(section_count: u16) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1i32.to_le_bytes());
        raw.extend_from_slice(&2i32.to_le_bytes());
        raw.push(0);
        raw.extend_from_slice(&(-4i32).to_le_bytes());
        raw.extend_from_slice(&section_count.to_le_bytes());
        raw
    }

    /// Append a `[u16 len][utf8]` string, the way `write_str` does.
    fn push_wire_str(out: &mut Vec<u8>, s: &str) {
        let len = u16::try_from(s.len()).expect("test string fits a u16");
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    /// Close a body off: a one-entry biome palette, its indices, and no block entities.
    fn push_chunk_tail(raw: &mut Vec<u8>) {
        raw.push(1);
        push_wire_str(raw, "minecraft:plains");
        raw.extend_from_slice(&[0u8; 16]);
        raw.extend_from_slice(&0u16.to_le_bytes());
    }

    /// Decode a body that must be rejected, returning the error for inspection.
    fn decode_err(raw: &[u8], what: &str) -> String {
        match decode_chunk(&deflate(raw)) {
            Ok(_) => panic!("{what} must be rejected, but decoding succeeded"),
            Err(e) => e,
        }
    }

    #[test]
    fn decode_chunk_rejects_a_packed_length_larger_than_the_body() {
        // Twenty-odd bytes claiming four gigabytes of packed indices. The prefix has to be
        // checked against the bytes that actually remain, before anything is reserved for it.
        let mut raw = chunk_prologue(1);
        raw.push(2); // paletted
        raw.extend_from_slice(&1u16.to_le_bytes()); // palette of one
        push_wire_str(&mut raw, "minecraft:stone");
        raw.push(4); // bits_per_index(1)
        raw.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            raw.len() < 64,
            "hostile payload is only {} bytes",
            raw.len()
        );

        let err = decode_err(&raw, "a four-gigabyte dataLen");
        assert!(
            err.contains("packed data length") && err.contains("remain"),
            "the length prefix must be checked against the remaining bytes, got: {err}"
        );
    }

    #[test]
    fn decode_chunk_rejects_every_oversized_length_prefix() {
        // A section count of u16::MAX with no section bytes behind it.
        let section_count = chunk_prologue(u16::MAX);

        // A paletted section claiming 65535 palette entries.
        let mut palette_len = chunk_prologue(1);
        palette_len.push(2);
        palette_len.extend_from_slice(&u16::MAX.to_le_bytes());

        // A uniform section whose name claims 65535 bytes that are not there.
        let mut string_len = chunk_prologue(1);
        string_len.push(1);
        string_len.extend_from_slice(&1u16.to_le_bytes());
        string_len.extend_from_slice(&u16::MAX.to_le_bytes());

        // A biome palette of 255 entries with nothing behind it.
        let mut biome_palette_len = chunk_prologue(0);
        biome_palette_len.push(u8::MAX);

        // A block entity count of u16::MAX with nothing behind it.
        let mut block_entity_count = chunk_prologue(0);
        block_entity_count.push(1);
        push_wire_str(&mut block_entity_count, "minecraft:plains");
        block_entity_count.extend_from_slice(&[0u8; 16]);
        block_entity_count.extend_from_slice(&u16::MAX.to_le_bytes());

        let cases: [(Vec<u8>, &str); 5] = [
            (section_count, "section count"),
            (palette_len, "palette length"),
            (string_len, "string length"),
            (biome_palette_len, "biome palette length"),
            (block_entity_count, "block entity count"),
        ];

        for (raw, expected) in cases {
            assert!(
                raw.len() < 64,
                "{expected}: the hostile payload is only {} bytes",
                raw.len()
            );
            let err = decode_err(&raw, expected);
            assert!(
                err.contains(expected) && err.contains("remain"),
                "the {expected} prefix must be checked against the remaining bytes, got: {err}"
            );
        }
    }

    #[test]
    fn decode_chunk_never_panics_on_adversarial_bytes() {
        let payload = ChunkPayload {
            chunk_x: -3,
            chunk_z: 9,
            clipped: true,
            min_section_y: -4,
            sections: vec![
                SectionPayload::Empty,
                SectionPayload::Paletted {
                    palette: vec!["minecraft:stone".into(), "minecraft:dirt".into()],
                    indices: (0..SECTION_CELLS).map(|i| (i % 2) as u16).collect(),
                },
                SectionPayload::Uniform("minecraft:air".into()),
            ],
            biomes: biome_grid("minecraft:plains"),
            block_entities: vec![BlockEntityPayload {
                x: 5,
                y: 70,
                z: -2,
                kind: "sign".to_string(),
                data: serde_json::json!({ "lines": ["a", "b"] }),
            }],
        };

        // Recover the uncompressed body so it can be cut at every structural boundary.
        let encoded = encode_chunk(&payload).expect("encode");
        let mut body = Vec::new();
        DeflateDecoder::new(&encoded[..])
            .read_to_end(&mut body)
            .expect("inflate");
        assert_eq!(decode_chunk(&deflate(&body)).expect("round trip"), payload);

        // Every truncation is malformed, and every one must be an Err rather than a panic.
        for cut in 0..body.len() {
            assert!(
                decode_chunk(&deflate(&body[..cut])).is_err(),
                "a body truncated to {cut} bytes must be rejected"
            );
        }

        // Trailing junk is malformed too.
        let mut trailing = body.clone();
        trailing.extend_from_slice(&[0xAB; 7]);
        assert!(decode_err(&trailing, "trailing junk").contains("trailing"));

        // Invalid UTF-8 in a section name.
        let mut raw = chunk_prologue(1);
        raw.push(1);
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.extend_from_slice(&2u16.to_le_bytes());
        raw.extend_from_slice(&[0xFF, 0xFE]);
        assert!(decode_err(&raw, "a non-UTF-8 name").contains("UTF-8"));

        // A bits-per-index the palette does not justify, including 0 and values wider than a
        // u64 word. None of them may reach a shift.
        for bits in [0u8, 1, 3, 5, 17, 33, 64, 200, u8::MAX] {
            let mut raw = chunk_prologue(1);
            raw.push(2);
            raw.extend_from_slice(&1u16.to_le_bytes());
            push_wire_str(&mut raw, "minecraft:stone");
            raw.push(bits);
            raw.extend_from_slice(&0u32.to_le_bytes());
            let err = decode_err(&raw, "a mismatched bits-per-index");
            assert!(err.contains("bits per index"), "bits {bits} gave: {err}");
        }

        // A packed block that fits in the buffer but is the wrong size for 4096 values.
        let mut raw = chunk_prologue(1);
        raw.push(2);
        raw.extend_from_slice(&1u16.to_le_bytes());
        push_wire_str(&mut raw, "minecraft:stone");
        raw.push(4);
        raw.extend_from_slice(&8u32.to_le_bytes());
        raw.extend_from_slice(&[0u8; 8]);
        assert!(decode_err(&raw, "an eight-byte packed block").contains("packed data is"));

        // An unknown section kind byte.
        let mut raw = chunk_prologue(1);
        raw.push(9);
        assert!(decode_err(&raw, "kind byte 9").contains("unknown kind"));

        // An explicitly empty section palette.
        let mut raw = chunk_prologue(1);
        raw.push(2);
        raw.extend_from_slice(&0u16.to_le_bytes());
        assert!(decode_err(&raw, "an empty palette").contains("empty palette"));

        // A uniform section that declares a palette other than one.
        let mut raw = chunk_prologue(1);
        raw.push(1);
        raw.extend_from_slice(&3u16.to_le_bytes());
        assert!(decode_err(&raw, "a uniform palette of three").contains("uniform"));

        // An empty biome palette, and a biome index pointing past the palette.
        let mut raw = chunk_prologue(0);
        raw.push(0);
        assert!(decode_err(&raw, "an empty biome palette").contains("biome palette is empty"));
        let mut raw = chunk_prologue(0);
        raw.push(1);
        push_wire_str(&mut raw, "minecraft:plains");
        raw.extend_from_slice(&[7u8; 16]);
        assert!(decode_err(&raw, "biome index 7").contains("out of range"));

        // A block entity whose JSON is not JSON.
        let mut raw = chunk_prologue(0);
        raw.push(1);
        push_wire_str(&mut raw, "minecraft:plains");
        raw.extend_from_slice(&[0u8; 16]);
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.extend_from_slice(&0i32.to_le_bytes());
        raw.extend_from_slice(&0i32.to_le_bytes());
        raw.extend_from_slice(&0i32.to_le_bytes());
        push_wire_str(&mut raw, "sign");
        push_wire_str(&mut raw, "{not json");
        assert!(decode_err(&raw, "malformed entity JSON").contains("block entity JSON"));

        // Finally, a deterministic sweep of pseudo-random bodies. None may panic.
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for case in 0..2000u32 {
            let len = (next() % 96) as usize;
            let mut raw: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Half the cases keep a well-formed prologue so the fuzz reaches the section parser.
            if case % 2 == 0 {
                let mut prefixed = chunk_prologue((next() % 4096) as u16);
                prefixed.append(&mut raw);
                raw = prefixed;
            }
            let _ = decode_chunk(&deflate(&raw));
        }
    }

    #[test]
    fn decode_chunk_accepts_a_hand_built_body() {
        // The cases above must be rejected for the right reason, not because the hand-built
        // layout is wrong: the same builders also produce a body that decodes cleanly.
        let mut raw = chunk_prologue(1);
        raw.push(1);
        raw.extend_from_slice(&1u16.to_le_bytes());
        push_wire_str(&mut raw, "minecraft:stone");
        push_chunk_tail(&mut raw);

        let decoded = decode_chunk(&deflate(&raw)).expect("hand-built body must decode");
        assert_eq!(decoded.chunk_x, 1);
        assert_eq!(decoded.chunk_z, 2);
        assert_eq!(decoded.min_section_y, -4);
        assert_eq!(decoded.sections.len(), 1);
        assert_eq!(
            decoded.sections[0],
            SectionPayload::Uniform("minecraft:stone".to_string())
        );
        assert_eq!(decoded.biomes, biome_grid("minecraft:plains"));
        assert!(decoded.block_entities.is_empty());
    }

    // -- bit packing --------------------------------------------------------------------------

    #[test]
    fn four_bit_packing_has_an_exact_known_layout() {
        let mut indices = vec![0u16; SECTION_CELLS];
        indices[0] = 1;
        indices[5] = 1;
        indices[15] = 0xF;
        indices[16] = 2; // first value of the second word
        let data = pack_indices(&indices, 4);
        // 16 values per 64-bit word, 256 words.
        assert_eq!(data.len(), 256 * 8);
        let word0 = u64::from_le_bytes(data[0..8].try_into().unwrap());
        assert_eq!(word0, 1 | (1 << 20) | (0xF << 60));
        let word1 = u64::from_le_bytes(data[8..16].try_into().unwrap());
        assert_eq!(word1, 2);
        // And the very first byte on the wire is the low two values: 0x1 and 0x0.
        assert_eq!(data[0], 0x01);
        assert_eq!(unpack_indices(&data, 4, SECTION_CELLS).unwrap(), indices);
    }

    #[test]
    fn values_never_straddle_a_word_boundary() {
        for bits in [5u8, 9u8] {
            let bits_usize = usize::from(bits);
            let per_word = 64 / bits_usize;
            let max_value = (1u32 << bits) - 1;
            let indices: Vec<u16> = (0..SECTION_CELLS)
                .map(|i| ((i as u32) % (max_value + 1)) as u16)
                .collect();
            let data = pack_indices(&indices, bits);
            let word_count = SECTION_CELLS.div_ceil(per_word);
            assert_eq!(data.len(), word_count * 8, "{bits} bits: word count");

            let mask = (1u64 << bits) - 1;
            for (i, expected) in indices.iter().enumerate() {
                let word_index = i / per_word;
                let shift = (i % per_word) * bits_usize;
                assert!(
                    shift + bits_usize <= 64,
                    "{bits} bits: value {i} would straddle a word boundary"
                );
                let word = u64::from_le_bytes(
                    data[word_index * 8..word_index * 8 + 8].try_into().unwrap(),
                );
                assert_eq!(
                    ((word >> shift) & mask) as u16,
                    *expected,
                    "{bits} bits: value {i} reads back with one shift and one mask"
                );
            }

            // The leftover high bits of every word are padding and must be zero.
            let padding_shift = per_word * bits_usize;
            if padding_shift < 64 {
                for word_bytes in data.chunks_exact(8) {
                    let word = u64::from_le_bytes(word_bytes.try_into().unwrap());
                    assert_eq!(word >> padding_shift, 0, "{bits} bits: padding is zero");
                }
            }

            assert_eq!(unpack_indices(&data, bits, SECTION_CELLS).unwrap(), indices);
        }
    }

    #[test]
    fn bits_per_index_follows_the_ceiling_rule() {
        assert_eq!(bits_per_index(1), 4);
        assert_eq!(bits_per_index(16), 4);
        assert_eq!(bits_per_index(17), 5);
        assert_eq!(bits_per_index(32), 5);
        assert_eq!(bits_per_index(33), 6);
        assert_eq!(bits_per_index(256), 8);
        assert_eq!(bits_per_index(257), 9);
        assert_eq!(bits_per_index(512), 9);
        assert_eq!(bits_per_index(513), 10);
        // The wire format caps a palette at u16::MAX, but the function is public: an absurd
        // argument must saturate rather than shift `1usize` past the width of a usize.
        assert_eq!(bits_per_index(usize::MAX), 32);
    }

    #[test]
    fn unpack_rejects_a_wrong_sized_data_block() {
        let data = pack_indices(&[0u16; SECTION_CELLS], 4);
        assert!(unpack_indices(&data[..data.len() - 8], 4, SECTION_CELLS).is_err());
        assert!(unpack_indices(&data, 3, SECTION_CELLS).is_err());
        assert!(unpack_indices(&data, 17, SECTION_CELLS).is_err());
    }
}
