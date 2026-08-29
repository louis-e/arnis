# Stream mode: choosing a tile size

Stream mode generates a **tile** (`ARNIS_STREAM_TILE_SIZE`, 512 blocks by default) plus a **margin**
(`ARNIS_STREAM_MARGIN`, 128 blocks), throws the margin away, caches the inner tile and serves 16x16
chunks out of it. Tile size is the one knob that trades throughput against the stall a player feels
when they walk into terrain nobody has generated yet. This note records how that trade-off is
measured, and why the default is what it is.

---

> ## ⚠ THE NUMBERS BELOW HAVE NOT BEEN MEASURED
>
> Every cell in [Results](#5-results) and [Fixed vs marginal cost](#6-fixed-vs-marginal-cost) is a
> **PLACEHOLDER**. Nothing in those two tables is an observation, an estimate or a guess — they are
> empty on purpose, because a fabricated benchmark number is worse than no number at all.
>
> Fill them by running the harness:
>
> ```sh
> ARNIS_STREAM_BENCH=1 cargo run --release
> ```
>
> It needs network access (Overpass plus elevation tiles) and takes tens of minutes. It prints the
> finished table in Markdown, ready to paste over the placeholder rows.
>
> The [margin waste](#2-the-geometry-comes-first) table is *not* a placeholder: it is arithmetic, it
> is exact, and it already settles part of the question.

---

## 1. What is being traded

**Throughput.** A tile job costs a fixed amount (an Overpass round trip, an elevation fetch, the
flood-fill caches, the projection and ground grids) plus a marginal amount proportional to the
*padded* area it generates. Larger tiles amortise the fixed cost over more usable ground and waste
proportionally less of the padded area on margin.

**Time to first chunk.** A player stepping into a cold tile waits for that whole tile: the job runs
to completion before the first waiter is answered, and only then do the other chunks of that tile
become microsecond cache hits. Larger tiles make that one stall longer.

These pull in opposite directions, which is the entire reason this document exists.

## 2. The geometry comes first

Before any measurement, the margin already argues for large tiles. A tile of edge `t` with margin
`m` generates `(t + 2m)^2` to keep `t^2`, so the **effective margin waste ratio** is

```
waste = (t + 2m)^2 / t^2
```

At the default margin of 128 blocks (scale 1.0, one block = one metre):

| Tile | Generated (padded) | Usable km² | Padded km² | Waste | Chunks per tile |
| ---: | --- | ---: | ---: | ---: | ---: |
| 128 | 384 x 384 | 0.016 | 0.147 | **9.00** | 64 |
| 256 | 512 x 512 | 0.066 | 0.262 | **4.00** | 256 |
| 512 | 768 x 768 | 0.262 | 0.590 | **2.25** | 1024 |
| 1024 | 1280 x 1280 | 1.049 | 1.638 | **1.56** | 4096 |

A 128-block tile does nine times the work it keeps. That alone rules the small end out: at 128 the
server spends 89% of every job generating ground it is about to discard, and it pays the fixed
per-job cost 16 times as often per square kilometre as at 512.

Note also where the curve flattens. Going 128 → 256 → 512 removes waste quickly (9 → 4 → 2.25).
Going 512 → 1024 removes much less (2.25 → 1.56, a 1.44x improvement) while multiplying the padded
area of a single job — and therefore the expected stall — by 2.78x.

## 3. What the harness measures

`src/stream/bench.rs`, for every (area, tile size) pair:

| Metric | Meaning |
| --- | --- |
| `ttfc_ms` | **Time to first chunk.** Wall clock from writing `RequestChunk` to reading the first `ChunkData` frame back off the socket. This is the whole tile job, and it is the latency a player actually feels. |
| `total_ms` | First chunk plus every remaining chunk of the tile, requested one at a time. Everything after the first is a cache hit. |
| `ms_per_km2` | `total_ms` divided by the tile's usable km². The throughput number. |
| `peak_rss_bytes`, `rss_delta_bytes` | Peak process RSS while the tile is in flight, absolute and minus the baseline sampled immediately before the request. **Peak memory per in-flight tile.** |
| `margin_waste` | `padded_area / usable_area`, from the table above. |
| `bytes_served`, `chunks` | Compressed payload written, and how many chunks came out of the tile. |
| `fixed_ms`, `marginal_ms_per_km2`, `fit_r2` | Per area: a least-squares fit of `ttfc = fixed + marginal * padded_area` across the tile sizes, which is what separates the per-job overhead from the per-area cost. |

**Measured through the real path, not simulated.** Each configuration starts an actual stream
server on an ephemeral loopback port, reads the session token out of the discovery file like any
client would, completes a real `Hello` handshake and requests real chunks over TCP. Time to first
chunk is therefore socket-to-socket, with encoding, DEFLATE and framing included.

**One fresh server per configuration**, so the measured request is a genuine cold first touch and
never a cache hit left behind by the previous tile size.

**Areas.** Three real places, one per density class, because element count per square kilometre is
what actually makes tile size behave differently:

| Area | Kind | Coordinates | Why |
| --- | --- | --- | --- |
| Manhattan Midtown | `dense_city` | 40.7549, -73.9840 | About as dense as OpenStreetMap gets: skyscraper footprints, building parts, a full street grid. |
| Levittown NY | `suburban` | 40.7251, -73.5143 | The archetypal post-war suburb — uniform detached houses on curved streets. |
| Arnis, Germany | `rural` | 54.6300, 9.9300 | Germany's smallest town and this project's namesake: a few hundred buildings on a fjord, so terrain, water and land cover dominate. |

Tile sizes: **128, 256, 512, 1024**. These are meaningful only because `src/tile.rs` now assigns
elements correctly at every tile size; before that fix `assign_elements_to_tiles` silently dropped
elements at any size but 512, and a benchmark run against it would have measured the bug.

## 4. How to run it

There is no CLI flag — stream mode does not have one, and this is not a user-facing feature. The
harness is gated on an environment variable, in the style of the other `ARNIS_*` switches, and
reports through the same `[BENCHMARK] <label>=<value>` stderr lines as `src/bench.rs`, so existing
tooling parses it unchanged.

```sh
# The full matrix: 3 areas x 4 tile sizes.
ARNIS_STREAM_BENCH=1 cargo run --release

# One configuration, which is the only way the memory column is trustworthy (see below).
ARNIS_STREAM_BENCH=1 ARNIS_STREAM_BENCH_AREAS=manhattan_midtown \
ARNIS_STREAM_BENCH_SIZES=512 cargo run --release

# Run every configuration twice and keep the second, so the shared on-disk OSM/elevation caches
# are warm for the measured pass and the numbers describe generation rather than bandwidth.
ARNIS_STREAM_BENCH=1 ARNIS_STREAM_BENCH_WARM=1 cargo run --release
```

Extract the metrics with the same grep as any other Arnis benchmark:

```sh
ARNIS_STREAM_BENCH=1 cargo run --release 2>&1 | grep '^\[BENCHMARK\]'
```

**Wiring.** `run_from_env()` returns `false` immediately unless `ARNIS_STREAM_BENCH=1`, so the hook
in `main()` is one line and costs a normal run nothing:

```rust
if crate::stream::bench::run_from_env() {
    return;
}
```

**Caveats to respect when reading the output.**

- *The network is in the measurement.* A cold run pays Overpass and elevation fetches; a warm one
  hits the on-disk cache under `dirs::cache_dir()/arnis-tile-cache`, which is shared across runs.
  Only compare sizes measured in the same state.
- *Memory is process RSS.* Generation is serialised onto a single worker, so exactly one tile is in
  flight and the peak-minus-baseline delta is that tile's cost — but the allocator does not return
  freed pages promptly, so the delta shrinks for every configuration after the first *in the same
  process*. Quote the memory column only from single-configuration runs.
- *Chunks after the first are requested sequentially*, one round trip each. That is not how a mod
  behaves, and `total_ms` therefore carries a small per-chunk loopback cost that a pipelining client
  would not pay. It is the same overhead at every tile size, so the comparison stands.

## 5. Results

**PLACEHOLDER — nothing here has been measured.** Replace these rows wholesale with the Markdown
table the harness prints.

| Area | Kind | Tile | Padded km² | Waste | Chunks | TTFC ms | Total ms | ms/km² | Peak RSS MB | Tile RSS MB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Manhattan Midtown | dense_city | 128 | 0.147 | 9.00 | 64 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Manhattan Midtown | dense_city | 256 | 0.262 | 4.00 | 256 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Manhattan Midtown | dense_city | 512 | 0.590 | 2.25 | 1024 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Manhattan Midtown | dense_city | 1024 | 1.638 | 1.56 | 4096 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Levittown NY | suburban | 128 | 0.147 | 9.00 | 64 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Levittown NY | suburban | 256 | 0.262 | 4.00 | 256 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Levittown NY | suburban | 512 | 0.590 | 2.25 | 1024 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Levittown NY | suburban | 1024 | 1.638 | 1.56 | 4096 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Arnis Germany | rural | 128 | 0.147 | 9.00 | 64 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Arnis Germany | rural | 256 | 0.262 | 4.00 | 256 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Arnis Germany | rural | 512 | 0.590 | 2.25 | 1024 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Arnis Germany | rural | 1024 | 1.638 | 1.56 | 4096 | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |

Machine, OS, core count and date of the run belong here too — none of these numbers mean anything
without them.

## 6. Fixed vs marginal cost

`ttfc = fixed + marginal * padded_area`, fitted across the four tile sizes of each area.

**PLACEHOLDER — nothing here has been measured.**

| Area | Fixed ms per job | Marginal ms per padded km² | R² |
| --- | ---: | ---: | ---: |
| Manhattan Midtown | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Levittown NY | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |
| Arnis Germany | PLACEHOLDER | PLACEHOLDER | PLACEHOLDER |

This split is the thing to read first, because it decides the argument:

- **If `fixed` dominates** at every size we test (`fixed >> marginal * padded_area`), the job cost is
  mostly round trips and setup, small tiles are catastrophic, and the pick should move up to 1024.
- **If `marginal` dominates**, time to first chunk is close to proportional to padded area, so a
  larger tile buys throughput at an almost exactly proportional cost in latency, and the pick is
  settled by the latency budget rather than by efficiency.
- **A low R²** means the linear model does not describe that area — most likely because network
  variance swamped the signal — and neither term should be quoted. Rerun with
  `ARNIS_STREAM_BENCH_WARM=1`.

## 7. The default, and why

**Default: `tile_size = 512`. Provisional, pending the measurement above.**

The reasoning, weighing the two sides explicitly:

**Against smaller tiles (128, 256).** The waste ratio is decisive and needs no measurement: 9.00 at
128 and 4.00 at 256, against 2.25 at 512. At 128 the server would do four times the work per usable
square kilometre that it does at 512, and pay the fixed per-job cost 16 times as often. Smaller
tiles do genuinely improve time to first chunk — that is the honest case for them — but they buy it
by making the server slower at everything, and a stall that recurs every 128 blocks of walking is
worse for a player than a longer stall every 512 blocks, even when each individual stall is shorter.

**Against larger tiles (1024).** Throughput keeps improving, but the returns have flattened: 1024
removes only 1.44x more waste than 512, while generating 2.78x the padded area in a single job.
If the marginal term dominates, that is a ~2.8x longer stall the first time a player crosses into
new terrain, for a 44% efficiency gain. It also costs about 4x the memory per in-flight tile, and
4x the RAM for each of the 16 tiles the cache holds by default.

**For 512.** It is where the waste curve turns: most of the benefit of large tiles, before the
latency and memory costs steepen. It is also one Anvil region (32x32 chunks), which is the grain the
rest of the pipeline is already banded, tested and tuned at — `DEFAULT_TILE_SIZE` in `src/tile.rs`
is 512 for exactly that reason, and at 512 the tile grid is bit-for-bit the region-aligned grid
production has always used. One stall then amortises over 1024 chunks, i.e. 512 blocks of walking in
any direction from the point of entry.

**This is a judgement about the shape of the curve, not a measurement**, and the measurement can
overturn it. The decision rule the numbers should be read against:

> Pick the **largest** tile size whose measured warm time to first chunk on the dense-city area stays
> inside the client's first-touch budget. Absent a measured budget, treat a few seconds as the
> ceiling: a client that prefetches one tile ahead can hide a stall of that order, and cannot hide a
> stall of tens of seconds.

Concretely: if 512 comes in comfortably under budget on Manhattan and 1024 does too, move the default
to 1024 — the throughput is free at that point. If 512 blows the budget on dense city, drop to 256
and accept 1.78x more work per usable square kilometre as the price of a playable first touch.
Record which of those the measurement showed when the tables above are filled in.

## 8. Configuration

The default is not baked in. All three tile-layer knobs are read from the environment at startup
(`src/stream/tiles.rs`) and reported in the `HelloOk` handshake, so a client always knows the size it
is actually getting:

| Variable | Default | Notes |
| --- | ---: | --- |
| `ARNIS_STREAM_TILE_SIZE` | 512 | Clamped to a positive multiple of 16, so tiles always break on chunk boundaries. |
| `ARNIS_STREAM_MARGIN` | 128 | Blocks of context generated around each tile and discarded. Never negative. Changing it changes every waste ratio in this document. |
| `ARNIS_STREAM_CACHE_TILES` | 16 | Finished tiles kept resident. Interacts with tile size: the cache's memory footprint scales with `tile_size^2 * cache_tiles`. |

```sh
# Serve 1024-block tiles.
ARNIS_STREAM_TILE_SIZE=1024 cargo run --release
```

Rerun the harness after any pipeline change that could move these numbers — a new generation stage, a
change to the margin, a different fetch path — and update the tables. That is what the harness is in
the repository for.
