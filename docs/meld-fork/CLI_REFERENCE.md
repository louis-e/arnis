# Meld Fork — CLI Reference

This page documents the CLI flags and source-level changes added by the
**Meld fork** (`Teddy563/arnis:feat/meld-fork`) on top of upstream
`louis-e/arnis` v2.7.0. Each flag is **opt-in**: omitting it preserves
upstream behaviour byte-for-byte.

The upstream README applies as-is. This file documents only the
delta.

---

## Flag inventory

| Flag | Type | Default | Added by | Intent |
|---|---|---|---|---|
| `--master-origin-lat <f64>` | `Option<f64>` | unset | PR 1 | Anchor block coords to a global lat origin so adjacent bbox runs tile cleanly. |
| `--master-origin-lng <f64>` | `Option<f64>` | unset | PR 1 | Same, longitude. Both must be set for the offset to apply. |
| `--elevation-min <f64>` | `Option<f64>` | unset | PR 2 | Force the global elevation floor (m) used by `scale_to_minecraft`. Stops Y seams between adjacent tiles. |
| `--elevation-max <f64>` | `Option<f64>` | unset | PR 2 | Same, ceiling. Both must be set for the lock to engage. |
| `--tile-invariant-rendering` | `bool` | `false` | PR 3 | Carry pre-clip bounds + polygon area on `ProcessedWay` so a building straddling two tiles renders identically in both. Off by default → upstream-style clipped-node decisions. |
| `--overpass-url <url[,url2,...]>` | `Vec<String>` | empty | PR 4 | Override the Overpass mirror pool. For self-hosted instances or LAN mirrors. |
| `--road-detail max\|compact\|none` | `String` | `max` | PR 5 | Skip pedestrian-grade highways + crossings + lane dividers at low scale. Trims the Overpass payload too. |

PR 3's salted-RNG-per-decision change is always on regardless of the
flag — it's a correctness property even for single-tile users (stops
upstream changes from cascading shifts across unrelated building
decisions) and toggling it requires duplicating ~150 lines around
an `if` branch we'd rather not maintain. The flag controls only the
unclipped-bounds reads.

---

## PR 1 — `--master-origin-lat / --master-origin-lng`

### What

Two optional CLI flags that anchor block-coordinate output to a global
latitude/longitude origin. When both are set, every Arnis run shifts
its bbox-to-block transform so the resulting `.mca` files land at
**globally-correct region indices** rather than always centring on
`(0, 0)`.

### Why

Upstream Arnis centres each run at world `(0, 0)` of the run's bbox.
Two adjacent bbox runs both write `r.0.0.mca`, `r.0.-1.mca`, etc.,
so they overlap each other instead of tiling. There's no way to do
country-scale generation without forking.

### Intended use

External schedulers (e.g. [Meld](https://github.com/Teddy563/meld))
that break a country into N adjacent cells and merge them into one
Minecraft world. The scheduler picks one global origin (typically the
country's centroid), then passes the same lat/lng pair on every
Arnis run.

### Example

```bash
# Origin = Bucharest centroid. Two adjacent tiles, same world.
ORIGIN="--master-origin-lat 44.4268 --master-origin-lng 26.1025"

arnis --bbox 44.40,26.05,44.45,26.10 --output-dir /tmp/world $ORIGIN
arnis --bbox 44.40,26.10,44.45,26.15 --output-dir /tmp/world $ORIGIN

# Both runs write into /tmp/world/region/ — non-overlapping .mca files.
ls /tmp/world/region/
```

### Math

```text
const M_PER_DEG_LAT = 111_320 m
let m_per_deg_lon = M_PER_DEG_LAT * cos(lat)
let dx_m = (bbox_sw_lon - origin_lng) * m_per_deg_lon
let dz_m = (origin_lat - bbox_sw_lat) * M_PER_DEG_LAT
let off_x = floor(dx_m * scale)
let off_z = floor(dz_m * scale)
xzbbox.translate(off_x, off_z)
```

### Source files touched

`src/args.rs`, `src/coordinate_system/transformation.rs`,
`src/coordinate_system/cartesian/xzbbox/xzbbox_enum.rs` (new
`from_points` constructor), `src/osm_parser.rs`,
`src/data_processing.rs`, `src/gui.rs`.

### Backward compatibility

Both flags optional. When omitted (or only one provided), behaviour
is **identical** to upstream — same single-tile, centred-at-origin
output.

---

## PR 2 — `--elevation-min / --elevation-max`

### What

Two optional CLI flags that lock the elevation-to-Y mapping to a
**global** range. When both are set, `scale_to_minecraft` uses them
as the reference range; per-tile auto-fit (current upstream
behaviour) is skipped.

### Why

Upstream `scale_to_minecraft` derives its elevation range from the
**current tile's grid**. Two adjacent tiles see different local
min/max → the same real-world metre maps to different Minecraft Y
values → height seam at the canonical boundary.

Even with PR 1 in place, terrain elevation doesn't align without a
shared Y reference frame. PR 1 + PR 2 together solve the multi-tile
correctness problem.

### Intended use

The scheduler surveys the entire region's elevation **once** (e.g.
fetch every AWS Terrarium tile covering the bbox at zoom 10, decode,
filter no-data sentinels, take min/max). Every Arnis run then uses
that locked range.

### Example

```bash
# Romania surveyed range: floor 60 m, ceiling 350 m.
LOCK="--elevation-min 60 --elevation-max 350"

# Every tile uses the same Y mapping — no seams.
arnis --bbox 44.40,26.05,44.45,26.10 --output-dir /tmp/world $LOCK
arnis --bbox 44.40,26.10,44.45,26.15 --output-dir /tmp/world $LOCK
```

### ⚠ Caller responsibility

**Filter AWS no-data sentinels before computing the range.** Raw RGB
`(0, 0, 0)` decodes to −32768 m. Passing a sentinel through this
flag would compress 33 km of range into ~360 blocks and flatten the
world. Recommended clamp to a sane Earth range like `[−500, 9000]`.

### Source files touched

`src/args.rs`, `src/elevation/postprocess.rs`,
`src/elevation/mod.rs`, `src/ground.rs`,
`src/data_processing.rs`, `src/gui.rs`.

### Backward compatibility

Both flags optional. When omitted, the existing per-tile auto-fit
runs unchanged.

---

## PR 3 — `--tile-invariant-rendering`

### What

One opt-in CLI flag (default `false`). Three structural changes:

1. **Pre-clip bounds + polygon area** carried on every `ProcessedWay`
   so building decisions don't read the bbox-clipped node list.
2. **Decision-aware skyscraper / footprint / diagonality / start-Y**
   logic that uses the pre-clip metrics when present.
3. **Salted RNG streams** (`element_rng_salted(id, salt)`) — one
   stream per decision instead of one shared stream, so unrelated
   branch divergence cannot shift downstream block choices.

### Why

A building straddling a tile boundary today renders **differently**
in each tile because:

- Skyscraper detection (`height ≥ 2 × longest_side`) reads clipped
  longest_side → category flips per tile → palette flips.
- Cached footprint size, diagonality, start-Y offset all reduce over
  clipped nodes → all per-tile-variant.
- A single shared `element_rng(id)` stream advances through every
  decision; if upstream adds an extra `rng.gen()` anywhere, every
  downstream block choice shifts.

Even single-tile users benefit: salted RNG isolates each decision so
upstream changes don't cascade.

### Intended use

Multi-tile schedulers (Meld) pass `--tile-invariant-rendering`
whenever they're generating a tile that's part of a shared world.
Single-tile users omit it and get upstream-style clipped-node
decisions.

The Meld scheduler emits the flag automatically when `origin.set`
is true (i.e. multi-tile mode). User can disable via
`settings.tile_invariant_rendering = false` if they want
byte-identical-to-upstream output for the unclipped-bounds reads.

### Example

```bash
# Single-tile, upstream behaviour (no flag):
arnis --bbox 44.43,26.10,44.44,26.11 --output-dir /tmp/single

# Multi-tile, identical building rendering across cells:
ORIGIN="--master-origin-lat 44.43 --master-origin-lng 26.10"
arnis --bbox 44.43,26.10,44.44,26.11 --output-dir /tmp/world $ORIGIN --tile-invariant-rendering
arnis --bbox 44.43,26.11,44.44,26.12 --output-dir /tmp/world $ORIGIN --tile-invariant-rendering
# Building straddling lon=26.11 renders with same blocks in both cells.
```

### Backward compatibility

- Default `false` → unclipped-bounds reads disabled → upstream
  v2.7.0-identical decisions for skyscraper / footprint /
  diagonality / start-Y.
- `unclipped_bounds` and `unclipped_polygon_area` are `Option`.
  When flag off, parse_data() + synthetic-ring constructors set
  both to `None`. The 4 decision sites already fall back to
  clipped-nodes via existing `if let Some(...)` branches.
- **Salted RNG (10 streams) stays on regardless of flag.** Toggling
  it would require duplicating ~150 lines of `BuildingStyle::resolve()`
  around an `if` branch we'd rather not maintain. Salted RNG
  produces **different block sequences** vs upstream's shared-stream
  path; single-tile renders see different (still deterministic)
  palettes than upstream v2.7.0 even with the flag off. If upstream
  maintainers want byte-for-byte v2.7.0 preservation including RNG,
  the salted path could move behind a separate `--tile-invariant-rng`
  sub-flag in review. Happy to split.

### Source files touched

`src/osm_parser.rs` (struct fields + helpers),
`src/overture.rs`, `src/element_processing/landuse.rs`,
`src/element_processing/leisure.rs`,
`src/element_processing/natural.rs`,
`src/element_processing/buildings.rs` (RNG salts + 4 decision
sites).

---

## PR 4 — `--overpass-url`

### What

Override the public Overpass mirror pool with a comma-separated list
of custom URLs. Arnis hits them in priority order; falls through to
the next on failure.

### Why

`fetch_data_from_overpass` hard-codes a list of public Overpass
mirrors. Public mirrors throttle ~2 connections/IP. Batch jobs
(country-scale generation) hit rate limits hard — 12-16 parallel
workers see ~90% fetch failures and spend most time in retry-backoff.

### Intended use

Self-hosted Overpass instance on LAN, paid mirror, or any custom
endpoint. Especially useful with the `--overpass-url` flag pointing
at `http://localhost:12345/api/interpreter` for a local Docker
[wiktorn/overpass-api](https://github.com/wiktorn/Overpass-API)
container.

### Example

```bash
# Default (public mirrors):
arnis --bbox 44.40,26.05,44.45,26.10 --output-dir /tmp/test

# Self-hosted Overpass on localhost:12345:
arnis --bbox 44.40,26.05,44.45,26.10 --output-dir /tmp/test \
      --overpass-url http://localhost:12345/api/interpreter

# Failover chain (LAN mirror first, public fallback):
arnis --bbox 44.40,26.05,44.45,26.10 --output-dir /tmp/test \
      --overpass-url http://lan-host:12345/api/interpreter,https://overpass-api.de/api/interpreter
```

### Source files touched

`src/args.rs`, `src/retrieve_data.rs`, `src/main.rs`,
`src/gui.rs`, `src/test_utilities.rs`.

### Backward compatibility

Empty list (default) → behaviour identical to upstream. Random-probe
+ arnis-api + shuffled-fallbacks chain runs unchanged.

---

## PR 5 — `--road-detail max | compact | none`

### What

Three-value flag controlling which OSM highway features are fetched
+ rendered:

| Value | Behaviour |
|---|---|
| `max` (default) | Render every `highway`, `footway`, `cycleway`, `crossing`, lane divider. Identical to upstream. |
| `compact` | Drop `footway`, `path`, `cycleway`, `steps`, `corridor`, `pedestrian`, `platform`, `bus_stop`, `crossing` markers. Lane dividers kept (they run along the road, not stacked across blocks). Vehicle-grade roads only. **Trims the Overpass query** so the payload is ~30-50% smaller too. |
| `none` | Skip every highway entirely. For terrain-only worlds. |

### Why

At scale<0.7 (1 block ≥ 1.5 m) Arnis's full-detail rendering
collapses footways + crosswalks + lane dividers onto the same few
blocks as the underlying road. Result: dotted-checker noise at every
intersection.

`compact` mode drops the visually-broken features at fetch + render
time. Vehicle roads still render legibly because their geometry is
chunky enough.

### Intended use

Scheduler picks `compact` automatically when `scale < 0.7`,
otherwise `max`. Manual override available for users who want full
detail at low scale (or no roads at all).

### Example

```bash
# Default (max, current upstream behaviour):
arnis --bbox 44.43,26.10,44.44,26.11 --output-dir /tmp/max --scale 0.5

# Compact (pedestrian noise gone — clean asphalt + lane stripes):
arnis --bbox 44.43,26.10,44.44,26.11 --output-dir /tmp/compact --scale 0.5 \
      --road-detail compact

# Terrain-only (no roads at all):
arnis --bbox 44.43,26.10,44.44,26.11 --output-dir /tmp/none --scale 0.5 \
      --road-detail none
```

### Source files touched

`src/args.rs`, `src/retrieve_data.rs` (Overpass query gate),
`src/element_processing/highways.rs` (per-element skip),
`src/main.rs`, `src/gui.rs`, `src/test_utilities.rs`.

### Backward compatibility

`max` is the default → omitting the flag preserves upstream
behaviour exactly.

---

## Combined example: country-scale Romania run

A scheduler emits this command for every cell:

```bash
arnis \
  --bbox 44.40,26.05,44.45,26.10 \
  --output-dir /path/to/master_world \
  --scale 0.5 \
  --master-origin-lat 45.9432 --master-origin-lng 24.9668 \
  --elevation-min -440 --elevation-max 2157 \
  --overpass-url http://localhost:12345/api/interpreter \
  --road-detail compact
```

Result: every tile lands at a globally-correct `.mca` index, terrain
heights align across boundaries, OSM data fetches from a self-hosted
Overpass mirror at zero rate-limit, and pedestrian-grade road noise
is suppressed at the chosen scale.

---

## Verifying the binary

```bash
arnis --help | grep -E "master-origin|elevation-min|elevation-max|overpass-url|road-detail"
```

Expect six flags in the output (two `master-origin-*`, two
`elevation-*`, one `overpass-url`, one `road-detail`).

If any flag is missing, the corresponding patch from the Meld fork
didn't apply during refresh. See `meld_arnis_fork/REFRESH.md` in the
Meld repo.

---

## Source change quick reference

| File | Lines added | Carries which PR |
|---|---|---|
| `src/args.rs` | +18 (4 flags + 1 enum-string flag) | 1, 2, 4, 5 |
| `src/coordinate_system/transformation.rs` | ~30 | 1 |
| `src/coordinate_system/cartesian/xzbbox/xzbbox_enum.rs` | +13 (`from_points` ctor) | 1 |
| `src/osm_parser.rs` | +60 (struct fields + helpers) | 1, 3 |
| `src/overture.rs` | +3 | 3 |
| `src/element_processing/buildings.rs` | ~120 | 3 |
| `src/element_processing/{landuse,leisure,natural}.rs` | +1 each | 3 |
| `src/element_processing/highways.rs` | +50 (skip helper + lane gate) | 5 |
| `src/elevation/mod.rs` | +6 | 2 |
| `src/elevation/postprocess.rs` | ~25 | 2 |
| `src/ground.rs` | ~29 | 2 |
| `src/data_processing.rs` | +6 | 1, 2 |
| `src/retrieve_data.rs` | ~50 (override URLs + road-detail gate) | 4, 5 |
| `src/main.rs` | +2 | 4, 5 |
| `src/gui.rs` | +5 | 1, 2, 4, 5 |
| `src/test_utilities.rs` | +1 | 4, 5 |

Total: ~1200 net changed lines across 18 files.
