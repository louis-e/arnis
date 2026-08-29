# Arnis stream protocol

`PROTOCOL_VERSION = 1` · Arnis 3.1.0 · status: prototype

Normative specification of the TCP protocol Arnis speaks in stream mode. Arnis is the authority on this
protocol: a client is anything that can open a loopback socket, speak JSON and inflate a DEFLATE stream.
Blocks are named with vanilla Minecraft identifiers because that is the vocabulary Arnis generates in, but
nothing else in this document assumes a Minecraft client. Where behaviour described here only makes sense
for a Minecraft client it is marked *typical client behaviour* and is not part of the contract.

---

## 1. Overview

Normal Arnis runs generate a bounded area and write a world to disk. Stream mode instead keeps Arnis
resident as a local server: a client declares how its world maps real elevation onto vertical block
coordinates, registers one or more real-world anchor points, and then pulls 16x16 block chunks on demand
as its players move.

Internally the server does not generate per chunk. It generates a **tile** (512x512 blocks by default)
plus a **margin** (128 blocks) so that features crossing the tile edge — roads, rivers, building
footprints — are resolved with their full context, discards the margin, caches the inner tile, and serves
chunks out of that cache. The first chunk request into a cold tile is slow (network fetches plus
generation); the remaining 1023 chunks of that tile are cache hits. Clients should assume first-touch
latency of seconds and steady-state latency of microseconds.

**Transport.** A plain TCP stream on `127.0.0.1`, port chosen at start and published in the discovery
file (section 2). No TLS, no framing library, no HTTP. Arnis binds the loopback interface only.

**Request/response model.** The client sends requests; the server answers. The server never initiates a
transaction of its own. Every request carries a client-chosen `request_id` (u64), and every message the
server sends in response echoes that id.

- Replies **may arrive out of order**. The server processes cheap control requests immediately while a
  generation job is running, so a `Ping` sent after a `RequestChunk` will usually be answered first.
  Correlate strictly by `request_id`; never by arrival order.
- A request produces zero or more `Progress` messages followed by **exactly one** terminal message
  (`ChunkData`, `ColumnData`, `Pong`, `HelloOk`, `HelloError` or `Error`). `Prefetch` and `Cancel` are
  the exceptions: they produce a terminal message only on failure.
- `request_id` values must be unique among the client's in-flight requests. Reuse after the terminal
  message is legal. Monotonically increasing ids are recommended because they make logs readable.

**Byte order.** Every integer in this protocol, in framing and in the chunk payload alike, is
**little-endian**. Signed integers are two's complement. Floats are IEEE-754 doubles, carried inside JSON.

**Encoding split.** The control plane is JSON (UTF-8, no BOM) for every message except `ChunkData`. This
is deliberate: control traffic is low-volume and being able to read it in a packet dump is worth more
than the bytes it costs. The data plane — `ChunkData` — is a compact binary body, DEFLATE-compressed.

---

## 2. Discovery and authentication

On start, the stream server writes a discovery file. On clean stop it deletes it.

| Platform | Path |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/arnis/stream.json`, or `~/.config/arnis/stream.json` |
| macOS | `~/Library/Application Support/arnis/stream.json` |
| Windows | `%APPDATA%\arnis\stream.json` |

The path is the platform config directory (`dirs::config_dir()`) plus `arnis/stream.json`.

```json
{
  "port": 51730,
  "pid": 44011,
  "protocolVersion": 1,
  "arnisVersion": "3.1.0",
  "sessionToken": "9f2c41ab77de40518c3e6d0a1b95cf72"
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `port` | u16 | TCP port on `127.0.0.1` the server is listening on |
| `pid` | u32 | Process id of the Arnis instance that wrote the file |
| `protocolVersion` | u32 | Protocol version this server speaks; always `1` today |
| `arnisVersion` | string | Arnis release, for diagnostics and client-side compatibility notes |
| `sessionToken` | string | 32 lowercase hex characters (128 random bits), regenerated on every start |

**Stale files.** A crashed Arnis leaves the file behind. On start, if the file exists, the server checks
whether the recorded `pid` belongs to a live process. If it does not, the file is overwritten without any
warning — this is the normal recovery path, not an error condition. If it does, another Arnis instance is
already serving stream mode; the second instance reports that and does not take over the file.

Clients should apply the mirror-image rule: if connecting to the recorded port fails (connection
refused), treat the file as stale and report "Arnis is not running", not "Arnis refused the connection".

**Authentication.** The client must echo `sessionToken` verbatim in the `Hello` message. A mismatch is
answered with `HelloError { code: "bad_token" }` and the connection is closed.

This is **not a security boundary** and must not be presented to users as one. Any local process that can
open a loopback socket can also read the discovery file. The token exists to stop accidental cross-talk —
two Arnis instances, a client left running from a previous session, a stale port reused by something
else — and to turn "silently talking to the wrong process" into one clear error message. Do not build
authorisation on top of it.

---

## 3. Framing

Every message, in both directions, is one frame:

```
+--------+--------+--------+--------+
|          payload_len : u32        |   little-endian, EXCLUDES this 9-byte header
+--------+--------+--------+--------+
| msg_type : u8   |
+--------+--------+--------+--------+--------+--------+--------+--------+
|                        request_id : u64                              |
+--------+--------+--------+--------+--------+--------+--------+--------+
|  payload : payload_len bytes                                         |
+----------------------------------------------------------------------+
```

| Offset | Size | Field | Notes |
| --- | --- | --- | --- |
| 0 | 4 | `payload_len` | u32 LE. Counts payload bytes only. A payload-less message is not legal; JSON messages with no fields carry `{}` (2 bytes). |
| 4 | 1 | `msg_type` | See section 4. |
| 5 | 8 | `request_id` | u64 LE. Chosen by the client; echoed by the server on every message belonging to that request. |
| 13 | `payload_len` | `payload` | UTF-8 JSON for every type except `ChunkData` (130), which is raw DEFLATE. |

The total frame size is therefore `payload_len + 9`.

A reader must read exactly 9 bytes, then exactly `payload_len` bytes, and must not assume that one
`read()` returns one frame — TCP will split and coalesce frames freely.

**Size cap.** A frame whose `payload_len` exceeds 16 MiB is rejected: the server answers
`Error { code: "bad_request" }` and closes the connection, because after an oversized length field the
stream can no longer be resynchronised. No legitimate message comes close to this.

### Worked example: Ping / Pong

`Ping` with `request_id = 7`, payload `{}`:

```
02 00 00 00   09   07 00 00 00 00 00 00 00   7B 7D
|----------|  |-|  |---------------------|   |---|
payload_len   type  request_id (7)            "{}"
= 2                 = 9 (Ping)
```

11 bytes on the wire. The reply is the same shape with `msg_type = 0x86` (134, `Pong`) and the same
`request_id`:

```
02 00 00 00   86   07 00 00 00 00 00 00 00   7B 7D
```

---

## 4. Message catalogue

### 4.1 Type bytes

| Byte | Name | Direction | Payload |
| --- | --- | --- | --- |
| 1 | `Hello` | client → server | JSON |
| 2 | `QueryElevationRange` | client → server | JSON |
| 3 | `AddAnchor` | client → server | JSON |
| 4 | `RequestChunk` | client → server | JSON |
| 5 | `RequestColumn` | client → server | JSON |
| 6 | `Locate` | client → server | JSON |
| 7 | `Prefetch` | client → server | JSON |
| 8 | `Cancel` | client → server | JSON |
| 9 | `Ping` | client → server | JSON |
| 128 | `HelloOk` | server → client | JSON |
| 129 | `HelloError` | server → client | JSON |
| 130 | `ChunkData` | server → client | DEFLATE binary |
| 131 | `ColumnData` | server → client | JSON |
| 132 | `Progress` | server → client | JSON |
| 133 | `Error` | server → client | JSON |
| 134 | `Pong` | server → client | JSON |

All JSON objects use `camelCase` field names. Unknown fields in a received object are ignored; absent
optional fields take the stated default. A message type byte the receiver does not know is a protocol
error: the server answers `Error { code: "bad_request" }`, a client should log and close.

**Type 131 is the generic JSON reply.** Despite the historical name `ColumnData`, message 131 carries the
reply to `QueryElevationRange`, `AddAnchor`, `Locate` **and** `RequestColumn`. Every 131 body carries a
`kind` discriminator (`"elevationRange"`, `"anchor"`, `"locate"`, `"column"`) so that a client can parse
the body without consulting its own request table. Correlation still happens through `request_id`;
`kind` exists so that a mismatch between the two is detectable instead of silently misparsed.

### 4.2 `Hello` (1) — client → server

Must be the first message on a connection. Any other message before it is answered with
`Error { code: "bad_request" }` and the connection is closed. A second `Hello` on the same connection is
also a `bad_request`.

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `protocolVersion` | u32 | yes | Protocol version the client speaks. See section 9. |
| `clientName` | string | yes | Free-form identifier, e.g. `"arnis-bridge"`. Logged. |
| `clientVersion` | string | yes | Client's own version string. Logged. |
| `sessionToken` | string | yes | Must equal the discovery file's `sessionToken`. |
| `config` | `GenConfig` | yes | Generation settings for the whole session. Fixed once. |
| `vertical` | `VerticalMapping` | yes | Vertical mapping for the whole session. Fixed once. |
| `anchors` | `[AnchorSpec]` | yes | Anchors the client already has. May be empty. |

`GenConfig`:

| Field | Type | Req. | Default | Meaning |
| --- | --- | --- | --- | --- |
| `scale` | f64 | yes | — | Blocks per metre. `1.0` is real size. Everything in this document assumes `1.0`; other values multiply all horizontal block distances, including anchor radii. |
| `fillground` | bool | yes | — | Fill the column below the surface with stone down to the terrain floor instead of leaving it hollow. |
| `interior` | bool | yes | — | Generate building interiors. |
| `use3d` | bool | yes | — | Use 3D building data (roof shapes, building parts) where available. |
| `overture` | bool | yes | — | Additionally fetch Overture Maps data. |
| `canopyHeight` | bool | yes | — | Use canopy-height raster data for tree heights. |
| `terrainOnly` | bool | yes | — | Terrain only: skip OpenStreetMap and Overture entirely. |
| `flatGround` | bool | no | `false` | Objects on flat ground: skip elevation, land cover and canopy. Terrain is flat at `seaLevelY`. Combined with `localOsmFile` this is the only configuration that generates without any network access. Mutually exclusive with `terrainOnly`, which would leave nothing to generate. |
| `localOsmFile` | string? | no | `null` | Absolute path to a local OSM extract. When set, tile generation reads vector data from this file instead of querying Overpass. |

`localOsmFile` is the seam for offline and bulk generation: all vector fetching for a tile goes through a
single function, so a file reader can replace the network path without any other change. When it is set,
the server performs no Overpass request at all, and `Progress { stage: "fetching_osm" }` still fires but
completes without network I/O.

`VerticalMapping` — see section 6:

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `minY` | i32 | yes | Lowest block Y of the client's world. Must be a multiple of 16. |
| `height` | i32 | yes | Total vertical extent in blocks. Must be a multiple of 16, at most 4064. |
| `seaLevelY` | i32 | yes | Block Y that represents 0 m elevation. Must satisfy `minY < seaLevelY < minY + height`. |
| `verticalScale` | f64 | yes | Blocks per metre of real elevation. `1.0` is real relief; `0.5` halves it. Must be finite and greater than 0. |

Additional constraint: `minY + height <= 2032`.

`AnchorSpec`:

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `id` | u32 | yes | Client-side anchor id. Unique within the session. |
| `lat` | f64 | yes | WGS84 latitude in degrees, `-85.0511 .. 85.0511`. |
| `lon` | f64 | yes | WGS84 longitude in degrees, `-180 .. 180`. |
| `mcX` | i32 | yes | Block X the anchor's lat/lon maps to. |
| `mcZ` | i32 | yes | Block Z the anchor's lat/lon maps to. |
| `radiusM` | f64 | yes | Patch radius in metres. Must be `> 0` and `<= 500000`. |

Anchors sent in `Hello` are taken as given — the client is restoring state it persisted earlier, and the
server must reproduce the same world it produced before, so it does not recompute `mcX`/`mcZ`. It does
validate them: duplicate ids, out-of-range values, or overlapping patches (section 5) are rejected with
`HelloError { code: "invalid_anchors" }`.

### 4.3 `HelloOk` (128) — server → client

| Field | Type | Meaning |
| --- | --- | --- |
| `arnisVersion` | string | Server version, e.g. `"3.1.0"`. |
| `protocolVersion` | u32 | Protocol version the server speaks. |
| `tileSize` | u32 | Tile edge in blocks, default `512`. Always a multiple of 16. |
| `maxInFlight` | u32 | Maximum concurrent generation-bearing requests, default `4`. See section 8. |
| `capabilities` | [string] | Optional feature names the server supports. Unknown entries must be ignored. |

`capabilities` is the forward-compatibility channel within a protocol version: features added later that
do not change existing message shapes are announced here rather than by bumping `PROTOCOL_VERSION`.
Clients must not require any particular entry to be present.

### 4.4 `HelloError` (129) — server → client

| Field | Type | Meaning |
| --- | --- | --- |
| `reason` | string | Human-readable explanation, suitable for showing to a user. |
| `code` | string | One of the codes below. |

| Code | Cause |
| --- | --- |
| `version_mismatch` | `protocolVersion` is not one the server speaks. |
| `bad_token` | `sessionToken` does not match the discovery file. |
| `invalid_vertical_mapping` | `vertical` violates a constraint in section 6. |
| `invalid_anchors` | An `AnchorSpec` is malformed, duplicated, or its patch overlaps another. |

The server closes the connection after sending `HelloError`.

### 4.5 `QueryElevationRange` (2) — client → server

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `lat` | f64 | yes | Centre latitude. |
| `lon` | f64 | yes | Centre longitude. |
| `radiusM` | f64 | yes | Radius in metres to sample. |

May be sent before `Hello` is possible only in the sense that it needs no anchors — it still requires a
completed handshake. Replies on type 131 with `kind: "elevationRange"`:

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `"elevationRange"` |
| `minElevationM` | f64 | Lowest sampled elevation in metres. |
| `maxElevationM` | f64 | Highest sampled elevation in metres. |
| `recommendedMinY` | i32 | Suggested `VerticalMapping.minY`, already section-aligned and within limits. |
| `recommendedHeight` | i32 | Suggested `VerticalMapping.height`. |
| `recommendedSeaLevelY` | i32 | Suggested `VerticalMapping.seaLevelY`. |

The recommendation includes headroom above the highest terrain for structures and vegetation and a
margin below the lowest for the terrain floor. A client is free to ignore it; the constraints in
section 6 are what is actually enforced.

### 4.6 `AddAnchor` (3) — client → server

Registers a new anchor mid-session and asks the server where it belongs.

| Field | Type | Req. | Default | Meaning |
| --- | --- | --- | --- | --- |
| `id` | u32? | no | next free | Client-chosen id. If omitted the server assigns the lowest unused id. |
| `lat` | f64 | yes | — | WGS84 latitude. |
| `lon` | f64 | yes | — | WGS84 longitude. |
| `radiusM` | f64? | no | `500000` | Patch radius in metres, at most `500000`. |

Unlike `Hello`, `AddAnchor` does **not** take `mcX`/`mcZ`: deriving them is the server's job (section 5).
Replies on type 131 with `kind: "anchor"` and the full resolved anchor:

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `"anchor"` |
| `id` | u32 | Assigned or echoed id. |
| `lat` | f64 | Echoed latitude. |
| `lon` | f64 | Echoed longitude. |
| `mcX` | i32 | Derived block X of the anchor. |
| `mcZ` | i32 | Derived block Z of the anchor. |
| `radiusM` | f64 | Effective radius. |

The client must persist the reply and send it back verbatim as an `AnchorSpec` in the next session's
`Hello`. Failure modes: `Error { code: "bad_request" }` for a duplicate id, an out-of-range value, or a
patch that would overlap an existing one.

### 4.7 `RequestChunk` (4) — client → server

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `chunkX` | i32 | yes | Chunk X, i.e. `block_x >> 4`. |
| `chunkZ` | i32 | yes | Chunk Z, i.e. `block_z >> 4`. |
| `anchorId` | u32? | no | Patch to resolve the chunk against. If omitted, the server resolves the patch that contains the chunk. |

Replies with `ChunkData` (130), optionally preceded by `Progress` messages. Failure modes:
`unknown_anchor` (an `anchorId` that was never registered), `out_of_patch` (the chunk lies outside every
patch), `generation_failed`, `busy`, `cancelled`.

A chunk inside a patch with nothing in it is a valid result: the server returns a `ChunkData` whose
sections are all air rather than an error. The server never invents filler terrain for a chunk it did not
generate — absent content is air, and the client decides what to do with it. *Typical client behaviour:*
treat an all-air reply as "nothing here", not as "generation is incomplete".

### 4.8 `RequestColumn` (5) — client → server

Cheap surface probe. Answers "what is the surface height at this block column" without transferring a
chunk. Useful for placing a spawn point or a teleport target.

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `x` | i32 | yes | Block X. |
| `z` | i32 | yes | Block Z. |
| `anchorId` | u32? | no | Patch to resolve against; omitted means "whichever patch contains it". |

Replies on type 131 with `kind: "column"`:

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `"column"` |
| `x` | i32 | Echoed block X. |
| `z` | i32 | Echoed block Z. |
| `surfaceY` | i32 | Block Y of the highest non-air block in that column. |
| `clipped` | bool | `true` if the true surface lay outside the client's vertical range and `surfaceY` is the clamped value. |

Answering may require generating the containing tile, so `RequestColumn` counts against `maxInFlight` and
can return `busy`, `generation_failed`, `out_of_patch` or `unknown_anchor` like `RequestChunk`.

### 4.9 `Locate` (6) — client → server

Geocodes a place name and, if it falls inside a registered patch, reports where that is in block
coordinates.

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `query` | string | yes | Free-form place name, e.g. `"Arnis, Germany"`. |

Replies on type 131 with `kind: "locate"`:

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | `"locate"` |
| `found` | bool | Whether the query resolved to a place at all. |
| `lat` | f64 | Resolved latitude. Meaningless when `found` is `false`. |
| `lon` | f64 | Resolved longitude. Meaningless when `found` is `false`. |
| `anchorId` | u32? | Anchor whose patch contains the result, or `null` if none does. |
| `mcX` | i32? | Block X of the result inside that patch, or `null`. |
| `mcZ` | i32? | Block Z of the result inside that patch, or `null`. |
| `note` | string | Human-readable remark, e.g. the full resolved display name, or why no patch matched. |

`found: false` is a successful reply, not an error — the query simply matched nothing. The geocoding
service being unreachable is a different outcome and produces `Error { code: "geocoding_unavailable" }`.
Clients must handle a resolved location that lies outside every patch (`anchorId: null`) by offering to
create an anchor with `AddAnchor`, not by treating it as a failure.

### 4.10 `Prefetch` (7) — client → server

A hint that chunks around a point will be needed soon, so the server can warm the tile cache.

| Field | Type | Req. | Default | Meaning |
| --- | --- | --- | --- | --- |
| `chunkX` | i32 | yes | — | Centre chunk X. |
| `chunkZ` | i32 | yes | — | Centre chunk Z. |
| `radiusChunks` | u32? | no | `4` | Chebyshev radius in chunks around the centre to warm. |
| `anchorId` | u32? | no | resolved | Patch to resolve against. |

`Prefetch` produces **no reply on success** and does not count against `maxInFlight`. The server may
ignore it entirely — when the generation worker is busy with real requests, dropping hints is correct
behaviour. It may emit `Error` for a malformed request (`bad_request`, `unknown_anchor`). A client must
never wait on a `Prefetch`; issue it and move on.

### 4.11 `Cancel` (8) — client → server

| Field | Type | Req. | Meaning |
| --- | --- | --- | --- |
| `requestId` | u64? | no | The in-flight request to cancel. If omitted, cancels all in-flight requests on this connection. |

Note that `requestId` here is a *field*, referring to another request; the `Cancel` frame's own header
`request_id` is its own, and is used only if the server has to report a `bad_request` against it.

`Cancel` itself gets no reply. The cancelled request terminates with `Error { code: "cancelled" }` on its
own `request_id`. See section 8 for what "cancelled" actually guarantees.

### 4.12 `Ping` (9) / `Pong` (134)

Both carry the payload `{}` (2 bytes). `Ping` is answered by `Pong` with the same `request_id`. `Ping` is
handled on the connection's reader path and is never queued behind generation, so it measures liveness
and round-trip time, not server load.

### 4.13 `Progress` (132) — server → client

Sent zero or more times before a request's terminal message, on that request's `request_id`.

| Field | Type | Meaning |
| --- | --- | --- |
| `stage` | string | `"fetching_osm"`, `"fetching_elevation"`, `"generating"` or `"encoding"`. |
| `detail` | string | Human-readable detail, e.g. `"tile 3,-7"`. Not machine-parsable; do not match on it. |

Stages are informational and are not guaranteed to occur, to occur in order, or to occur exactly once. A
cache-hit chunk emits none at all. Clients must not use `Progress` for control flow — only the terminal
message ends a request.

### 4.14 `Error` (133) — server → client

| Field | Type | Meaning |
| --- | --- | --- |
| `code` | string | Machine-readable code from the table in section 8. |
| `message` | string | Human-readable explanation. |

`Error` is terminal for the request it names. It never closes the connection; the only messages that do
are `HelloError` and an unrecoverable framing violation.

### 4.15 `ChunkData` (130) — server → client

Binary, DEFLATE-compressed. Layout in section 7.

---

## 5. Coordinate model

### 5.1 Anchors and patches

An **anchor** binds one real-world point to one block position:

```
anchor = (id, lat, lon, mcX, mcZ, radiusM)
```

The disc of radius `radiusM` metres around `(mcX, mcZ)` is that anchor's **patch**. All generation
happens inside a patch. A chunk outside every patch is not "empty" — it is `out_of_patch`, an error,
because the server has no defined mapping for it.

The default radius is 500 km, and 500 km is also the hard maximum. Section 5.3 explains why.

### 5.2 In-patch geometry: local transverse Mercator

Inside a patch, geographic coordinates are converted with a **transverse Mercator projection centred on
that anchor's own lat/lon**, with scale factor 1 on the central meridian and a spherical Earth of radius
`R = 6371000 m`. With `φ`/`λ` the point in radians and `φ0`/`λ0` the anchor:

```
B = cos(φ) · sin(λ − λ0)
E = R · atanh(B)                              easting, metres
N = R · ( atan2( tan(φ), cos(λ − λ0) ) − φ0 ) northing, metres
```

and the block position, with `s = GenConfig.scale` blocks per metre:

```
x = mcX + round(E · s)
z = mcZ − round(N · s)        north is −Z
```

The inverse, needed by clients that must turn a block position back into lat/lon:

```
D   = N/R + φ0
φ   = asin( sin(D) / cosh(E/R) )
λ   = λ0 + atan2( sinh(E/R), cos(D) )
```

At `scale = 1.0`, one block is one metre in both axes, everywhere in the patch. Geometry is undistorted:
a 30 m building is 30 blocks on every side regardless of latitude, and a square city block is square.

### 5.3 Why not Web Mercator inside a patch

Web Mercator is conformal but its scale factor grows as `1/cos(φ)`. Projecting building outlines with it
and reading the result as blocks would inflate everything by that factor:

| Latitude | Place | `1/cos(φ)` | A 30 m building becomes |
| --- | --- | --- | --- |
| 0° | Equator | 1.00 | 30 blocks |
| 40.7° N | New York | 1.32 | 40 blocks |
| 48.0° N | Munich / Paris | 1.49 | 45 blocks |
| 60.0° N | Oslo | 2.00 | 60 blocks |

The error is not a constant that could be divided out once: it is different in every city, and within one
large patch it varies across the patch. A world built that way is wrong in a way players notice — doorways
and street widths do not match the buildings around them, and the same building imported at two latitudes
comes out at two different sizes.

Transverse Mercator has the opposite distribution: its scale factor is `1/sqrt(1 − B²)`, which is exactly
1 on the anchor's meridian and grows with east-west distance from it. At 500 km east or west,
`B ≈ 0.0785` and the factor is about **1.003** — three parts in a thousand, a third of a block over a
hundred. That is the reason for the 500 km cap: it is the distance at which the local projection is still
visually exact.

### 5.4 Placing a new anchor: Web Mercator, snapped

The projection above is only defined relative to an anchor, so it cannot say where a *new* anchor goes.
For that, and only for that, Arnis uses **Web Mercator with a global origin at (0°, 0°)**:

```
xm = R · λ
zm = −R · ln( tan( π/4 + φ/2 ) )
```

then snaps to the tile grid (`tileSize` from `HelloOk`, 512 blocks by default):

```
mcX = round( xm / tileSize ) · tileSize
mcZ = round( zm / tileSize ) · tileSize
```

This gives patches a roughly correct global arrangement — Paris ends up north-east of Madrid, Tokyo far to
the east — without letting Web Mercator's scale distortion anywhere near the geometry inside a patch. The
inter-patch distances are stretched by the same `1/cos(φ)` factor, so two patches are further apart in
blocks than they are in reality; that discrepancy lands entirely in the empty space between patches, where
nothing is generated and nothing measures it. Snapping to the tile grid keeps every patch's tile lattice
aligned with every other's, which makes tile boundaries and cache keys consistent across the world.

### 5.5 Overlap is rejected

Two patches with different anchors define two different, incompatible mappings for the same blocks. The
server therefore **rejects overlapping patches** rather than picking a winner. Two anchors overlap when

```
distance( (mcX₁, mcZ₁), (mcX₂, mcZ₂) )  <  (radiusM₁ + radiusM₂) · scale
```

in blocks. An overlap in `Hello` yields `HelloError { code: "invalid_anchors" }`; an overlap in
`AddAnchor` yields `Error { code: "bad_request" }`. Clients that want two nearby places in one world
should use one anchor with a radius large enough to cover both, not two anchors.

---

## 6. Vertical model

### 6.1 The mapping

`VerticalMapping` fixes, for the whole session, how metres of real elevation become block Y:

```
y = seaLevelY + round( elevation_m · verticalScale )
```

The mapping is **absolute**, not relative to the terrain in view. Two tiles generated hours apart, or on
opposite sides of a patch, produce heights on the same scale and join seamlessly. There is no per-area
normalisation and no auto-fit.

| Field | Constraint |
| --- | --- |
| `minY` | `minY % 16 == 0` and `minY >= -2032` |
| `height` | `height % 16 == 0` and `height <= 4064` |
| `minY + height` | `<= 2032` |
| `seaLevelY` | `minY < seaLevelY < minY + height` |
| `verticalScale` | finite, `> 0` |

Violations are rejected at handshake time with `HelloError { code: "invalid_vertical_mapping" }`. The
server does not clamp a bad mapping into a good one, because the client has already created its world by
then and a silently different mapping would produce terrain that does not match.

### 6.2 Why the limits are what they are

These are engine limits, not tuning parameters, and no server setting can raise them.

- **`minY + height <= 2032`** (highest legal block Y is 2031). Positions are packed into a single 64-bit
  integer with **12 bits for Y**, giving an absolute range of −2048..2047.
- **`minY >= -2032`**, the mirror of the ceiling. `height <= 4064` bounds how *many* sections a world
  spans but says nothing about **where** that span sits, so a floor below −2032 anchors it outside the
  signed byte: `minY = -4048` gives a bottom section index of `-4048 >> 4 = -253`, which wraps to `3`,
  and the server's section range then reports a minimum above its maximum. Blocks are written into
  wrapped sections and the client receives terrain with instructions to place it thousands of blocks
  from where it belongs, so a deeper floor is rejected at the handshake rather than clamped.
- **`height <= 4064`.** The vertical section index is a **signed byte**, so sections run −128..127. One
  section of headroom is reserved at each end for lighting data below the floor and above the ceiling,
  leaving 254 usable sections: `254 × 16 = 4064`.
- **Multiples of 16** because the unit of storage is a 16x16x16 section; a floor or ceiling in the middle
  of a section corrupts every index derived from it.

### 6.3 The trade-off the client must make

The two properties below cannot both hold, and the client must choose before it creates its world:

1. **`seaLevelY = 0`.** Block Y then *equals* real elevation in metres (at `verticalScale = 1.0`). Reading
   a coordinate tells you the altitude; comparing two tells you the real height difference; contour data
   and OSM `ele` tags need no conversion. The cost: terrain is capped at **2031 m**. That covers the vast
   majority of inhabited land but excludes alpine terrain — the Alps, the Rockies, the Andes, the
   Himalaya all clip.
2. **Full altitude coverage.** Reaching 4000 m of terrain requires pushing `minY` down to about −2032 and
   putting sea level well below zero. Property 1 is gone: Y no longer reads as an altitude, and every
   tool, command and coordinate readout needs a mental offset.

There is no third option that keeps both. A client aimed at cities should take option 1; a client aimed at
mountain ranges should take option 2 and expose the offset in its UI. `verticalScale < 1.0` is the escape
hatch that compresses relief to fit, at the cost of no longer being real relief.

### 6.4 Clipping is reported, never silent

When terrain would exceed the client's declared range, the server clamps it to the range **and says so**:
the affected `ChunkData` frame sets **bit 0 of the `flags` byte**, and a `RequestColumn` reply sets
`clipped: true`. It never quietly rescales, never shifts the mapping, and never changes it mid-session —
any of which would make previously generated chunks disagree with new ones.

*Typical client behaviour:* surface the flag once per session ("terrain above 2031 m was flattened") rather
than per chunk.

### 6.5 Why `QueryElevationRange` exists

A client must size its world **before** creating it, because vertical extent cannot be changed afterwards
without invalidating everything already generated. `QueryElevationRange` samples the elevation data for a
region and returns the real minimum and maximum plus a fitted recommendation, so a client can pick the
smallest range that actually covers the terrain instead of defaulting to the maximum.

Oversizing is expensive, and the costs are structural rather than marginal:

- **Skylight propagation dominates.** Light is computed over the full vertical extent of the world. A
  4064-block world does roughly 10x the light work of a 400-block one, per chunk, forever — this is
  usually the single largest cost of a tall world.
- **Sections in memory.** 254 sections per chunk column instead of 24 — an order of magnitude more
  per-section bookkeeping for every loaded chunk, most of it empty air.
- **Light data in every chunk packet.** Light arrays are sized by world height, so every chunk sent to
  every viewer carries the larger payload whether or not anything is up there.
- **Level-of-detail generation.** Any distant-terrain or LOD system scales with vertical extent too, and
  is markedly more expensive over a tall world.

A world sized to its terrain and one sized to the maximum look identical and perform very differently.

---

## 7. Chunk encoding

The `ChunkData` (130) payload is a binary body **compressed with raw DEFLATE (RFC 1951)** — no zlib
header, no gzip header, no trailing checksum. The layout below describes the body **before** compression.
No other message is compressed.

Paletted encoding plus DEFLATE is sufficient and no further scheme is needed: the palette already removes
the bulk of the redundancy (a section is typically a handful of distinct block states), the index array is
bit-packed to the minimum width that palette needs, and DEFLATE then collapses the long runs that remain.

### 7.1 Header

| Field | Type | Meaning |
| --- | --- | --- |
| `chunk_x` | i32 | Chunk X (`block_x >> 4`). Echoes the request. |
| `chunk_z` | i32 | Chunk Z (`block_z >> 4`). Echoes the request. |
| `flags` | u8 | Bit 0: terrain in this chunk was vertically clipped (section 6.4). Bits 1–7 are reserved, sent as 0; clients must ignore unknown bits rather than reject the frame. |
| `min_section_y` | i32 | Section index of the lowest section present. Section `i` covers block Y `i*16 .. i*16+15`. |
| `section_count` | u16 | Number of sections that follow, contiguously, low to high. |

Sections outside `[min_section_y, min_section_y + section_count)` are air. The server sends a tight span,
not the client's full world height.

### 7.2 Sections

Each section starts with a `kind` byte:

| `kind` | Meaning | Body |
| --- | --- | --- |
| 0 | Entirely air | nothing follows |
| 1 | Uniform — one block state fills all 4096 cells | `[u16 palette_len = 1][palette entry]` |
| 2 | Paletted | `[u16 palette_len][palette entries][u8 bits_per_index][u32 data_len][data]` |

A palette entry is:

| Field | Type | Meaning |
| --- | --- | --- |
| `name_len` | u16 | Byte length of the UTF-8 string. |
| `name` | bytes | Blockstate string, section 7.3. |

For `kind = 2`:

| Field | Type | Meaning |
| --- | --- | --- |
| `bits_per_index` | u8 | `max(4, ceil(log2(palette_len)))`. |
| `data_len` | u32 | Length of `data` **in bytes**. Always a multiple of 8. |
| `data` | bytes | Bit-packed indices, `data_len / 8` little-endian u64 words. |

### 7.3 Blockstate strings

```
blockstate := namespaced_id [ "[" property { "," property } "]" ]
property   := key "=" value
```

- `namespaced_id` is always fully qualified, e.g. `minecraft:stone`.
- Properties are only present when the block has any. `minecraft:stone` carries no brackets.
- Keys are sorted ascending byte-wise, values are the plain vanilla forms (`north`, `true`, `8`).
- No whitespace anywhere.

```
minecraft:stone
minecraft:oak_stairs[facing=north,half=bottom]
minecraft:oak_leaves[distance=1,persistent=true,waterlogged=false]
```

One string per state, so a client can parse it with one split and a lookup. A non-Minecraft client maps
these identifiers to its own node types with a translation table; Arnis ships one such table for Luanti
in `src/luanti_block_map.rs`, which is a reasonable model for others.

### 7.4 Index packing

4096 cells per section, addressed in **YZX order**:

```
cell_index = (y & 15) * 256 + (z & 15) * 16 + (x & 15)
```

Indices are packed into little-endian u64 words, **without straddling word boundaries** (the Anvil 1.16+
rule — the simpler packing that wastes the leftover high bits of each word):

```
values_per_word = 64 / bits_per_index          integer division
word_count      = ceil(4096 / values_per_word)
word            = data[ cell_index / values_per_word ]
shift           = (cell_index % values_per_word) * bits_per_index
value           = (word >> shift) & ((1 << bits_per_index) - 1)
```

The high `64 % bits_per_index` bits of every word are zero and must be ignored. `data_len` is
`word_count * 8`. With the minimum `bits_per_index` of 4 that is 256 words, 2048 bytes.

### 7.5 Biomes

Immediately after the last section:

| Field | Type | Meaning |
| --- | --- | --- |
| `biome_palette_len` | u8 | Number of distinct biomes in this chunk. At least 1. |
| entries | — | `biome_palette_len` entries, `[u16 name_len][utf8 name]`, e.g. `minecraft:plains`. Never carries properties. |
| indices | 16 x u8 | One palette index per cell of a 4x4 horizontal grid. |

The grid is 4x4 in X/Z — each cell covers 4x4 blocks — and is **constant in Y**: the same 16 values apply
to every section of the column. Order is `grid_index = z * 4 + x`, matching the Z-major convention used by
the section packing.

### 7.6 Block entities

Last in the body:

| Field | Type | Meaning |
| --- | --- | --- |
| `count` | u16 | Number of entries. |

Then per entry:

| Field | Type | Meaning |
| --- | --- | --- |
| `x` | i32 | **Absolute** block X, not chunk-relative. |
| `y` | i32 | Absolute block Y. |
| `z` | i32 | Absolute block Z. |
| `kind_len` | u16 | Byte length of `kind`. |
| `kind` | bytes | Semantic kind: `sign`, `chest`, `banner`, `bed`, `item_frame`. |
| `json_len` | u16 | Byte length of `json`. |
| `json` | bytes | Small UTF-8 JSON object whose shape depends on `kind`. |

```json
{"lines":["Hauptstraße","","",""],"color":"black","facing":2}
```

| `kind` | Fields |
| --- | --- |
| `sign` | `lines` (array of exactly 4 strings, unused lines empty), `color` (dye colour name), `facing` (0–15 rotation step, 0 = south, increasing clockwise) |
| `chest` | `facing` (`north`/`south`/`east`/`west`), optional `items` |
| `banner` | `color` (dye colour name), optional `patterns` |
| `bed` | `color` (dye colour name) |
| `item_frame` | `facing`, optional `item` (namespaced item id) |

This is **never Minecraft NBT**, in any version or encoding. The payload is a semantic description of what
the object *is*; turning it into whatever the target engine stores is the client's job, and every
version-to-version difference in NBT layout stays on the client side of the wire. A client that does not
understand a `kind` must skip the entry using `json_len` and continue — the encoding is self-delimiting
precisely so that unknown kinds are survivable.

---

## 8. Backpressure, cancellation and errors

### 8.1 Serialised generation

All generation runs on **one worker thread**. This is not a placeholder for a thread pool: several
generation parameters are process-global state retuned per area, so two concurrent jobs would corrupt each
other's terrain. Parallelism exists *inside* a single job, which already saturates most of the machine's
cores. A second worker would not make the server faster; it would make it wrong.

The consequence for clients: chunk throughput is bounded by tile generation, and requests queue.

### 8.2 `maxInFlight`

`HelloOk.maxInFlight` (default 4) is the number of generation-bearing requests — `RequestChunk` and
`RequestColumn` — the client may have outstanding. Exceeding it does not block; the extra request is
answered immediately with:

```json
{"code":"busy","message":"too many requests in flight"}
```

`busy` is a routine flow-control signal, not a fault. The correct client response is to retry the same
request later, ideally after a terminal message has freed a slot. Do not treat it as an error worth
showing to a user, and do not retry in a tight loop.

`Ping`, `Cancel`, `Prefetch`, `Locate` and `QueryElevationRange` do not consume in-flight slots.

### 8.3 Cancellation

`Cancel` sets a flag that the generation pipeline checks **at stage boundaries** — between fetching,
generating and encoding. It does not interrupt work in progress. A cancel that arrives during a long
Overpass fetch takes effect when that fetch returns.

Guarantees:

- The cancelled request terminates with `Error { code: "cancelled" }` on its own `request_id`, or with its
  normal successful reply if it had already passed the last checkpoint. A client must accept either.
- Work already completed is not thrown away: a tile that finished generating stays in the cache and makes
  the next request for it fast.
- `Cancel` for an unknown or already-finished `request_id` is silently ignored.

### 8.4 Error codes

| Code | Meaning | Client action |
| --- | --- | --- |
| `unknown_anchor` | `anchorId` was never registered in this session. | Bug in the client's anchor bookkeeping. Re-register with `AddAnchor`. |
| `out_of_patch` | The requested chunk or column lies outside every registered patch. | Expected at patch edges. Treat as "no world here"; offer an anchor if the player intends to go there. |
| `generation_failed` | The tile job failed — network failure, malformed source data, internal error. `message` carries the detail. | Retryable, but back off; an immediate retry usually fails the same way. |
| `cancelled` | The request was cancelled at a stage boundary. | Normal. Do not surface. |
| `busy` | More than `maxInFlight` generation requests outstanding. | Retry later. Do not surface. |
| `bad_request` | Malformed frame, unknown message type, invalid field, out-of-range value, duplicate anchor id, overlapping patch. | Client bug. Log with the request id; do not retry unchanged. |
| `geocoding_unavailable` | The geocoding service could not be reached or refused the request. | Retry later; suggest entering coordinates directly. |

`HelloError` uses its own separate code set (section 4.4).

A client should handle every code it does not recognise as a non-fatal error on that request. New codes
may be added within protocol version 1.

---

## 9. Versioning

`PROTOCOL_VERSION` is `1`.

It appears in three places: the discovery file, the client's `Hello`, and the server's `HelloOk`.

**Negotiation** is a check, not a handshake. There is no version range and no downgrade: the client sends
the version it speaks, and the server either speaks it or does not. A mismatch is answered with
`HelloError { code: "version_mismatch" }`, whose `reason` names both versions, and the connection is
closed.

**What a client should do on mismatch.** Read `protocolVersion` from the discovery file *before*
connecting and compare it locally — that produces a clear message without a wasted connection. On a
mismatch, report which side is older and stop. Do not attempt to speak the other version, and do not
retry: the situation is a version skew between Arnis and the client, and only a user updating one of them
resolves it.

**What changes the version.** Any change to framing, to a type byte's meaning, to the `ChunkData` layout,
or to the required fields of an existing message. These are all breaking, and there is no compatibility
shim.

**What does not.** Adding an optional field to a JSON message, adding a new error code, adding a new
`Progress` stage, or adding a whole new feature announced through `HelloOk.capabilities`. Clients must
therefore ignore unknown JSON fields, unknown error codes, unknown `Progress` stages, unknown capability
strings, and unknown `flags` bits, rather than treating any of them as protocol violations. A client that
parses strictly will break on the next minor release; a client that ignores what it does not know will not.
