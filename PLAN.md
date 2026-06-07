# Arnis2: Parallelization, Global Projection & Multi-Generation Plan

## Table of Contents
1. [Executive Summary](#1-executive-summary)
2. [Current Architecture Analysis](#2-current-architecture-analysis)
3. [Proposal A: Spatial Parallelization](#3-proposal-a-spatial-parallelization)
4. [Proposal B: Global Projection & One-World Approach](#4-proposal-b-global-projection--one-world-approach)
5. [Proposal C: UI/UX Redesign for Multi-Generation](#5-proposal-c-uiux-redesign-for-multi-generation)
6. [How A, B, and C Interplay](#6-how-a-b-and-c-interplay)
7. [Recommended Implementation Order](#7-recommended-implementation-order)
8. [Technical Deep-Dives](#8-technical-deep-dives)
9. [Risks and Mitigations](#9-risks-and-mitigations)
10. [Alternatives Considered](#10-alternatives-considered)

---

## 1. Executive Summary

Three interconnected features are proposed:

- **A) Spatial Parallelization** — Subdivide the generation area into "tiles" and process them on multiple CPU cores to reduce generation time for large areas.
- **B) Global Projection** — Replace the current origin-at-(0,0) local coordinate system with a Web Mercator projection so that multiple generations land at the correct Minecraft coordinates and tile seamlessly.
- **C) Multi-Generation UI/UX** — Redesign the GUI to support generating multiple regions into one world, showing previously generated areas on the map, and managing worlds.

**Key insight: B is a prerequisite for C, and A is independent but benefits from the same spatial subdivision infrastructure that B introduces.** The recommended order is B → C → A, though A can be prototyped independently on a branch.

### Implementation Status (branch `feat/parallel-tiles-projection`)

> This document was written as a forward-looking design proposal. The current state on this branch:
>
> - **A) Spatial Parallelization — implemented.** `src/tile.rs` provides region-aligned 512-block tiling with halo zones and element-to-tile assignment. `src/data_processing.rs` extracts per-element dispatch into `process_element()` and runs it through a rayon tile-parallel path (areas ≥ 3 tiles) or a sequential path, merging per-tile `WorldEditor`s with authoritative bounds (`WorldToModify::merge`). Ground generation still runs once on the merged editor.
> - **B) Global Projection — implemented.** `src/projection/` provides the `Projection` trait and `WebMercatorProjection`; `CoordTransformer::with_projection` and `XZBBox::rect_from_min_max` allow non-(0,0)-origin bounding boxes. The `--projection {local,web_mercator}` CLI flag selects the mode (the GUI is currently pinned to `local`).
> - **C) Multi-Generation UI/UX — not yet implemented.**
>
> This branch has been rebased onto `main` at **v2.8.0**, which introduced several features after this plan was first written (Luanti/Minetest output, a 3D-models pipeline, bridge structures, baked Java lighting, per-chunk biomes, and a land-cover water-depth field). Both the sequential and tile-parallel element paths drive these new processors. The "Current Architecture Analysis" below describes the single-generation baseline that motivated the design and predates those v2.8.0 additions; treat its pipeline/file references as historical context rather than a current map of the code.

---

## 2. Current Architecture Analysis

### Processing Pipeline (Single-Threaded Bottleneck)

```
[1] Fetch OSM data          — HTTP, single-threaded
[2] Parse & transform       — Single-threaded, O(nodes + ways + relations)
[3] Fetch elevation          — Parallel (8 threads), download + Gaussian blur
[4] Pre-compute flood fills — Parallel (rayon), the main parallelized step
[5] Process elements        — *** SINGLE-THREADED *** — the main bottleneck
[6] Generate ground         — *** SINGLE-THREADED *** — iterates every block
[7] Save world              — Parallel (rayon over regions)
```

Steps 5 and 6 are sequential because all block writes go to a shared, non-thread-safe `WorldToModify` (nested `FnvHashMap` without locks).

### Coordinate System

- Geographic (WGS84 lat/lon) → Minecraft (x, z) via linear interpolation within the bounding box
- Bounding box always originates at Minecraft (0, 0)
- Scale: 1 block ≈ 1 meter (configurable via `--scale`)
- No global projection — each generation is an independent local coordinate system

### Memory Model

| Structure | Approx. Size | Notes |
|-----------|-------------|-------|
| `WorldToModify` | Grows throughout generation | All regions held in RAM until save |
| `BlockStorage::Uniform` | 1 byte/section | Empty/homogeneous sections |
| `BlockStorage::Full` | 4096 bytes/section | Mixed-block sections |
| `ElevationData` | `width × height × 4` bytes | Kept in `Arc<Ground>` for lifetime |
| `FloodFillCache` | Variable | Freed per-element after use |

**The biggest memory consumer is `WorldToModify`** — for a 10,000×10,000 block area with `--fillground`, this can reach hundreds of MB because every underground section becomes Full(Vec<Block>) before compaction.

### Randomness

All randomness is already deterministic:
- `element_rng(element_id)` — ChaCha8Rng seeded from OSM element ID
- `coord_rng(x, z, element_id)` — ChaCha8Rng seeded from coordinates + element ID
- `road_block(x, z)` — Pure coordinate bit-mixing hash, no RNG

**This is excellent for parallelization — randomness is already partition-independent.**

---

## 3. Proposal A: Spatial Parallelization

### 3.1 Concept

Subdivide the XZBBox into rectangular **tiles** of fixed size (e.g., 512×512 blocks = 1 Minecraft region). Each tile gets its own `WorldToModify` and processes its assigned elements independently on a separate CPU core. After all tiles complete, merge results into the final world.

### 3.2 Tile Size Selection

| Tile Size | Tiles for 5km² | Parallelism | Overhead | Recommendation |
|-----------|----------------|-------------|----------|----------------|
| 256×256 | ~76 | Very high | High merge cost | Too small |
| **512×512** | ~19 | Good | Moderate | **Recommended** — aligns with MC regions |
| 1024×1024 | ~5 | Low for small areas | Low | Good for very large areas |

**512×512 is recommended** because it aligns with Minecraft region boundaries, meaning each tile's `WorldToModify` maps cleanly to exactly one region file. This eliminates cross-region conflicts during merge entirely.

### 3.3 Element Assignment Strategy

Each OSM element must be assigned to one or more tiles for processing:

| Element Type | Assignment Strategy | Rationale |
|-------------|-------------------|-----------|
| **Buildings** | Centroid → single tile | Buildings are spatially compact; the tile that owns the centroid renders the entire building, including parts that extend beyond tile bounds |
| **Roads/Railways** | All tiles that the road intersects | Linear elements; each tile renders the segment within its bounds. Bresenham is deterministic, so overlapping writes produce identical blocks |
| **Trees (nodes)** | Trunk coordinate → single tile, with 8-block halo | Tree canopy extends ~7 blocks from trunk; the owning tile renders the full tree including canopy overflow |
| **Water areas** | Centroid → single tile | Large polygons; scanline fill is self-contained |
| **Landuse/Natural** | Centroid → single tile | Area polygons, flood-fill is self-contained |
| **Barriers/Fences** | All tiles intersected | Linear elements, same as roads |
| **Ground generation** | Each tile generates its own ground | Chunk-by-chunk, no cross-tile dependency |

### 3.4 Handling Boundary Issues

**Problem: A building at a tile boundary may place blocks in the neighboring tile's region.**

**Solution: Per-tile `WorldToModify` with unrestricted writes + deterministic merge.**

Each tile is *authoritative* for blocks within its strict 512×512 bounds. During the merge phase:

1. Iterate tiles in a fixed order (row-major: left→right, top→bottom)
2. For each tile's `WorldToModify`, iterate all regions/chunks/sections
3. For blocks **within the tile's strict bounds**: write to final world (authoritative)
4. For blocks **outside the tile's strict bounds**: write to final world only if the cell is still AIR (non-authoritative, fill-only)

This ensures:
- Buildings that extend past tile boundaries are rendered correctly (the centroid tile places all blocks)
- If two tiles both write to the same boundary coordinate, the authoritative tile's block wins
- Order is deterministic regardless of thread scheduling

**Problem: Trees at tile edges may have canopy cut off.**

**Solution: Halo zones.** Each tile's element filter includes tree nodes within 8 blocks outside the tile's strict bounds. The tree is fully rendered (trunk + canopy) by the tile that contains the trunk. Since the halo extends beyond the strict bounds, canopy leaves that fall into a neighbor's strict area are written as non-authoritative blocks.

**Problem: Flood-filled areas crossing tile boundaries.**

**Solution: The flood fill is pre-computed globally (step 4 in the pipeline) before tile assignment.** Each tile receives the pre-computed fill coordinates and filters them to its own bounds. The flood fill cache is already parallelized and shared read-only.

### 3.5 Ground Generation Parallelization

Ground generation currently iterates every (x, z) in the bounding box sequentially. With tiles:

- Each tile generates ground for its own 512×512 area independently
- Ground data (`Arc<Ground>`) is shared immutably across tiles
- No cross-tile dependency for ground blocks
- Vegetation placement uses `coord_rng(x, z, 0)` which is position-deterministic

### 3.6 Memory Impact

**Parallel tiles can actually *reduce* peak memory:**

- With N tiles processed in batches of `num_cpus`:
  - Only `num_cpus` tiles' `WorldToModify` buffers exist simultaneously
  - Each completed tile is merged into the final world and dropped
  - Peak memory ≈ final world + `num_cpus` × (single tile's WorldToModify)

- A single tile (512×512, ~1 region) with `--fillground` uses perhaps 4-8 MB
- 8 concurrent tiles: ~32-64 MB of temporary tile buffers
- This is likely *less* than the current approach where the entire world is built monolithically

**Optimization: Stream-to-disk.** Since tiles align with regions, completed regions can be written to `.mca` files and evicted from memory immediately, rather than accumulating all regions in RAM until the save phase. This could dramatically reduce peak memory for large worlds.

### 3.7 Implementation Sketch

```rust
fn generate_world_parallel(
    elements: Vec<ProcessedElement>,
    ground: Arc<Ground>,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    xzbbox: &XZBBox,
) -> WorldToModify {
    // 1. Subdivide world into 512x512 tiles
    let tiles = create_tiles(xzbbox, 512);

    // 2. Assign elements to tiles based on spatial intersection
    let tile_elements = assign_elements_to_tiles(&elements, &tiles);

    // 3. Process tiles in parallel batches
    let mut final_world = WorldToModify::default();

    for batch in tile_elements.chunks(rayon::current_num_threads()) {
        let batch_results: Vec<(TileBounds, WorldToModify)> = batch
            .into_par_iter()
            .map(|(tile_bounds, elements)| {
                let mut editor = WorldEditor::new(/* tile-local */);

                // Process elements assigned to this tile
                for element in elements {
                    process_element(&element, &mut editor, &ground, args, flood_fill_cache);
                }

                // Generate ground for this tile
                generate_ground_for_tile(&tile_bounds, &mut editor, &ground, args);

                (tile_bounds, editor.into_world())
            })
            .collect();

        // Merge batch results
        for (bounds, tile_world) in batch_results {
            merge_tile(&mut final_world, tile_world, &bounds);
        }
    }

    final_world
}
```

### 3.8 Expected Speedup

For an area producing N tiles on a machine with C cores:
- Current: ~T seconds (single-threaded elements + ground)
- Parallel: ~T/min(N, C) seconds for elements + ground, plus merge overhead
- Merge overhead: O(blocks written), sequential but fast (HashMap insertions)
- Realistic speedup: **3-6x on an 8-core machine** for areas ≥ 4 tiles

---

## 4. Proposal B: Global Projection & One-World Approach

### 4.1 The Problem

Currently, every generation starts at Minecraft (0, 0). If a user generates Berlin and then London, they are two separate worlds. There is no way to have them in the same Minecraft world at their correct relative positions.

### 4.2 Projection Options Evaluated

| Projection | Conformality | Distortion | Global Consistency | Complexity |
|-----------|-------------|-----------|-------------------|-----------|
| **Web Mercator** | Conformal | Area grows at poles | Yes (global) | Low |
| BTE Conformal Dymaxion | Near-conformal | Very low | Yes (global) | Very high |
| Equirectangular | No | High at poles | Yes (global) | Trivial |
| Local Tangent Plane (current) | Approximate | Low locally | No (each gen is independent) | Already done |

### 4.3 Recommendation: Web Mercator

**Web Mercator is the clear winner for this project.** It provides global coordinate consistency (enabling multi-generation tiling), conformal mapping (shapes look correct), trivial math, and natural alignment with the Leaflet/OSM tile system already used in the GUI. The alternatives were rejected for the following reasons:

- **BTE Dymaxion**: Extremely complex (icosahedral face detection, Newton's method inverse, 1.5 MB conformal vector field data). Produces coordinates millions of blocks from origin, which causes Minecraft client rendering issues. Fixed 1:1 scale eliminates the user's `--scale` option. Only beneficial for BTE project interoperability, which is not a goal.
- **Equirectangular**: Not conformal — shapes are visibly distorted, especially at higher latitudes where most users generate.
- **Local Tangent Plane (current)**: Works well for single generations but fundamentally cannot support multi-generation worlds because each generation starts at (0,0).

The `--projection local` option will be kept for backwards compatibility.

### 4.4 Web Mercator Details

**Forward projection (lat/lon → Minecraft x/z):**
```
x = R × (lon - lon_origin) × cos(lat_ref) × scale
z = -R × ln(tan(π/4 + lat/2)) × scale + z_offset
```

Where:
- R = 6,371,000 meters (Earth's mean radius)
- `lon_origin` and `lat_ref` are configurable (default: center of the first generation)
- `scale` is the user's `--scale` factor (default 1.0, so 1 block ≈ 1 meter at `lat_ref`)
- The negative sign on z ensures north points toward -Z in Minecraft (conventional)
- `z_offset` centers the first generation near (0, 0)

**Why Web Mercator:**
- It is conformal (shapes look correct at any latitude)
- The math is trivial (~10 lines of Rust)
- Leaflet/OpenStreetMap already use Web Mercator tiles, so the preview map naturally aligns
- At typical building latitudes (40-60°N), area distortion is only 1.3-2.4× — acceptable for city-scale and even metro-area-scale
- Every tool in the ecosystem (QGIS, Google Earth, OSM) speaks Mercator
- The `--scale` factor continues to work (adjusts blocks-per-meter ratio)
- Coordinates stay within reasonable ranges (not millions of blocks from origin) because a configurable origin centers the first generation near (0, 0)

**How tiling works:** Two generations at different locations get projected through the same Mercator function with the same origin. Their Minecraft coordinates are globally consistent — placing both in the same world produces the correct relative position automatically.

### 4.5 Coordinate System Changes

The core change: **XZBBox no longer starts at (0, 0).**

Currently, `XZBBox` is always `(0, 0) → (scale_factor_x, scale_factor_z)`. With Web Mercator, the bounding box will be at projected coordinates relative to the world origin (e.g., `(-2100, -1800) → (1500, 2200)` for a generation offset from the origin).

**What changes:**
- `XZBBox::min()` can return negative/large values
- `WorldEditor` bounds checking uses the projection-derived bbox
- Ground/elevation grid indices are offset from `xzbbox.min()` (already handled in `get_absolute_y`)
- Block coordinates in `WorldToModify` can be large negative numbers (already `i32`)
- Region file names include negative coordinates (already handled: `r.-17.-12.mca`)
- `level.dat` spawn point uses projected coordinates

**What stays the same:**
- `WorldToModify`'s FnvHashMap structure — works with any `(i32, i32)` keys
- Block storage — independent of coordinate magnitude
- Element processing — works with absolute coordinates already
- Save system — region/chunk/section derivation via bit shifts works for any i32

---

## 5. Proposal C: UI/UX Redesign for Multi-Generation

### 5.1 New Concepts

| Concept | Description |
|---------|------------|
| **Arnis World** | A Minecraft world directory that can contain multiple generations |
| **Generation** | A single execution: one bounding box → one set of Minecraft chunks |
| **Generation Record** | Metadata about a generation: bbox, timestamp, settings, projection |

### 5.2 World Management Flow

```
[Start Screen]
   ├── "New World" → Creates world directory, opens map
   └── "Open Existing World" → Shows list of Arnis worlds
           └── Opens map with previous generations shown as overlays

[Map Screen]
   ├── Previously generated areas shown as colored rectangles
   ├── User draws new selection rectangle
   ├── "Generate" button processes the new area
   └── New generation is appended to the world
```

### 5.3 Data Model

**`generations.json`** (stored in world directory):
```json
{
  "projection": "web_mercator",
  "projection_origin": { "lat": 52.52, "lon": 13.405 },
  "scale": 1.0,
  "generations": [
    {
      "id": "gen_001",
      "timestamp": "2026-04-13T10:30:00Z",
      "geo_bbox": { "min_lat": 52.50, "min_lon": 13.38, "max_lat": 52.54, "max_lon": 13.43 },
      "mc_bbox": { "min_x": -2100, "min_z": -1800, "max_x": 1500, "max_z": 2200 },
      "settings": { "terrain": true, "scale": 1.0, "roofs": true }
    },
    {
      "id": "gen_002",
      "timestamp": "2026-04-13T11:15:00Z",
      "geo_bbox": { "min_lat": 52.48, "min_lon": 13.35, "max_lat": 52.50, "max_lon": 13.38 },
      "mc_bbox": { "min_x": -4200, "min_z": 2200, "max_x": -2100, "max_z": 3800 },
      "settings": { "terrain": true, "scale": 1.0, "roofs": true }
    }
  ]
}
```

### 5.4 Map Overlay for Previous Generations

On the Leaflet map:
- Each previous generation is shown as a semi-transparent colored rectangle
- Color-coded: green = completed, yellow = in-progress
- Hovering shows generation metadata (date, settings)
- User can select a generation to re-generate or delete
- New selection rectangle is validated:
  - Warning if overlapping with existing generation (will overwrite those chunks)
  - Info showing total Minecraft coordinate range

### 5.5 Settings Constraints for Multi-Generation Worlds

When adding a generation to an existing world:
- **Projection is locked** — Must match the world's projection
- **Scale is locked** — Must match the world's scale
- **Terrain on/off is warned** — Mixing terrain and no-terrain causes elevation discontinuities
- Other settings (roofs, interiors, land cover) can vary per-generation

### 5.6 Terrain Stitching at Generation Boundaries

When two generations are adjacent, their terrain (elevation) may not match at the boundary because:
- Elevation data is fetched independently per generation
- Gaussian blur radius doesn't extend across generations
- Height scaling (compression to vanilla limits) may differ

**Solution: Boundary blending.**
- When generating a new area adjacent to an existing one, fetch elevation data for a slightly larger bbox (e.g., +50 blocks padding)
- At the boundary, linearly interpolate between the new terrain and the existing terrain in the world file
- This requires reading existing region files to detect already-generated chunks

---

## 6. How A, B, and C Interplay

```
                    ┌────────────────────┐
                    │  B: Global         │
                    │  Projection        │
                    │  (Foundation)      │
                    └──────┬─────────────┘
                           │
              ┌────────────┼────────────┐
              ▼                         ▼
   ┌──────────────────┐    ┌──────────────────┐
   │ C: Multi-Gen     │    │ A: Parallel      │
   │ UI/UX            │    │ Processing       │
   │ (Requires B)     │    │ (Independent)    │
   └──────────────────┘    └──────────────────┘
```

- **B enables C**: Without a global projection, multiple generations cannot be placed at correct Minecraft coordinates in the same world.
- **A is independent**: Parallelization works with or without a global projection. It speeds up single-generation processing.
- **A benefits from B's infrastructure**: The Web Mercator projection introduces a natural global coordinate grid, which can double as the parallelization tile grid.
- **C benefits from A**: When a user adds a new generation to an existing world, only the new area needs processing. Parallelization makes each generation faster.
- **A + B together**: The spatial tiles for parallelization can be defined in terms of the projected coordinate grid. Each tile is a fixed-size rectangle in projected coordinates. This means parallelization tiles are consistent across generations, aiding terrain stitching.

---

## 7. Recommended Implementation Order

### Phase 1: Web Mercator Projection Foundation (B)
**Effort: Medium | Impact: Enables everything else**

1. Add a `Projection` trait with `forward(lat, lon) -> (x, z)` and `inverse(x, z) -> (lat, lon)`
2. Implement `WebMercatorProjection` — ~50 lines of Rust
3. Modify `CoordTransformer` to use the projection instead of linear interpolation
4. Remove the assumption that XZBBox starts at (0, 0)
5. Add `--projection` CLI flag (default: `web_mercator`, option: `local` for backwards compat)
6. Store projection info in `metadata.json` / `generations.json`
7. Update the GUI settings to show projection choice

### Phase 2: Multi-Generation World Management (C)
**Effort: Medium-High | Impact: Major UX improvement**

1. Implement `generations.json` data model
2. Add "Open Existing World" flow to GUI
3. Show previous generations as overlays on the map
4. Lock projection/scale when appending to existing world
5. Support append-mode generation (write to existing world directory)
6. Implement overlap detection and warnings
7. Terrain stitching at generation boundaries (mark as experimental)
8. Update preview/map renderer for multi-generation worlds

### Phase 3: Spatial Parallelization (A)
**Effort: High | Impact: Major performance improvement**

1. Implement tile subdivision of XZBBox
2. Implement element-to-tile assignment (centroid, intersection, halo zone)
3. Create per-tile `WorldEditor` instances
4. Parallelize element processing loop with rayon
5. Parallelize ground generation per tile
6. Implement deterministic tile merge
7. Implement stream-to-disk optimization (write completed regions immediately)
8. Add `--threads` CLI flag and progress reporting per tile
9. Benchmark and tune tile size
10. Ensure deterministic output (bit-for-bit identical regardless of thread count)

### Phase 0 (Can start immediately, independent):
- Refactor `WorldToModify::merge()` function
- Add `Projection` trait infrastructure
- Write benchmarks for the current pipeline (needed to measure improvement)

---

## 8. Technical Deep-Dives

### 8.1 WorldToModify Merge Algorithm

```rust
impl WorldToModify {
    /// Merge another WorldToModify into self.
    /// `authoritative_bounds` defines the (x, z) range where `other` takes priority.
    /// Outside those bounds, `other` only writes to AIR blocks (non-authoritative).
    fn merge(&mut self, other: WorldToModify, authoritative_bounds: &TileBounds) {
        for (region_key, other_region) in other.regions {
            let self_region = self.regions.entry(region_key).or_default();
            for (chunk_key, other_chunk) in other_region.chunks {
                let self_chunk = self_region.chunks.entry(chunk_key).or_default();
                for (section_y, other_section) in other_chunk.sections {
                    match other_section.storage {
                        BlockStorage::Uniform(block) if block == AIR => continue,
                        BlockStorage::Uniform(block) => {
                            // All blocks in this section are the same
                            // Only merge if within authoritative bounds
                            // (simplified — full impl checks per-block)
                            let self_section = self_chunk.sections
                                .entry(section_y).or_default();
                            self_section.storage = BlockStorage::Uniform(block);
                        }
                        BlockStorage::Full(ref blocks) => {
                            let self_section = self_chunk.sections
                                .entry(section_y).or_default();
                            for (idx, &block) in blocks.iter().enumerate() {
                                if block == AIR { continue; }
                                let (x, y, z) = index_to_xyz(idx);
                                let world_x = /* compute from region/chunk/section */;
                                let world_z = /* compute from region/chunk/section */;

                                if authoritative_bounds.contains(world_x, world_z) {
                                    // Authoritative: always write
                                    self_section.set_block_at_index(idx, block);
                                } else {
                                    // Non-authoritative: only write to AIR
                                    if self_section.get_block_at_index(idx) == AIR {
                                        self_section.set_block_at_index(idx, block);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

### 8.2 Element-to-Tile Assignment

```rust
struct TileBounds {
    min_x: i32, min_z: i32,
    max_x: i32, max_z: i32,
}

impl TileBounds {
    fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x < self.max_x && z >= self.min_z && z < self.max_z
    }

    fn expanded(&self, halo: i32) -> TileBounds {
        TileBounds {
            min_x: self.min_x - halo,
            min_z: self.min_z - halo,
            max_x: self.max_x + halo,
            max_z: self.max_z + halo,
        }
    }
}

fn assign_elements_to_tiles(
    elements: &[ProcessedElement],
    tiles: &[TileBounds],
) -> Vec<Vec<&ProcessedElement>> {
    let mut tile_elements: Vec<Vec<&ProcessedElement>> = vec![Vec::new(); tiles.len()];

    for element in elements {
        match element {
            // Point elements (trees, amenities): assign to containing tile
            ProcessedElement::Node(node) => {
                for (i, tile) in tiles.iter().enumerate() {
                    if tile.expanded(8).contains(node.x, node.z) {
                        tile_elements[i].push(element);
                        break; // nodes go to exactly one tile
                    }
                }
            }
            // Area elements (buildings, landuse): assign by centroid
            ProcessedElement::Way(way) if is_area_element(way) => {
                let (cx, cz) = compute_centroid(way);
                for (i, tile) in tiles.iter().enumerate() {
                    if tile.contains(cx, cz) {
                        tile_elements[i].push(element);
                        break;
                    }
                }
            }
            // Linear elements (roads, railways): assign to all intersecting tiles
            ProcessedElement::Way(way) => {
                for (i, tile) in tiles.iter().enumerate() {
                    if way_intersects_bounds(way, &tile.expanded(4)) {
                        tile_elements[i].push(element);
                    }
                }
            }
            // Relations: assign by centroid of outer ring
            ProcessedElement::Relation(rel) => {
                let (cx, cz) = compute_relation_centroid(rel);
                for (i, tile) in tiles.iter().enumerate() {
                    if tile.contains(cx, cz) {
                        tile_elements[i].push(element);
                        break;
                    }
                }
            }
        }
    }

    tile_elements
}
```

### 8.3 Ground Generation Parallelization

Ground generation is naturally parallelizable because each (x, z) column is independent:

```rust
fn generate_ground_parallel(
    tiles: &[TileBounds],
    editors: &mut [WorldEditor],  // one per tile
    ground: &Arc<Ground>,
    args: &Args,
) {
    tiles.par_iter().zip(editors.par_iter_mut()).for_each(|(tile, editor)| {
        for chunk_x in (tile.min_x..tile.max_x).step_by(16) {
            for chunk_z in (tile.min_z..tile.max_z).step_by(16) {
                for x in chunk_x..min(chunk_x + 16, tile.max_x) {
                    for z in chunk_z..min(chunk_z + 16, tile.max_z) {
                        generate_ground_column(x, z, editor, ground, args);
                    }
                }
            }
        }
    });
}
```

### 8.4 Stream-to-Disk for Memory Reduction

Instead of accumulating all regions in RAM and saving at the end:

```rust
// After merging each tile batch:
for region_key in completed_region_keys {
    let region = final_world.regions.remove(&region_key).unwrap();
    save_single_region(region_key, &region, &world_path)?;
    // Region is dropped, freeing its memory
}
```

A region is "complete" when all tiles that could write to it have been processed. With row-major tile ordering and 512×512 tiles aligned to regions, a region is complete after the tile that owns it is merged.

---

## 9. Risks and Mitigations

### 9.1 Visual Seams at Tile Boundaries

**Risk:** Elements processed by different tiles may produce slightly different results at boundaries.

**Mitigation:**
- Deterministic RNG (already in place) ensures same element → same random choices regardless of tile
- Roads use coordinate-based bit-mixing hash (tile-independent)
- Authoritative bounds ensure no conflicting writes
- Trees use halo zones so canopies are complete

### 9.2 Flood Fill Cross-Tile Issues

**Risk:** A polygon that spans two tiles needs a complete flood fill, but each tile only sees part of the polygon.

**Mitigation:** Flood fills are pre-computed globally (step 4) before tile assignment. Each tile reads from the shared, immutable `FloodFillCache`. The fill coordinates are simply filtered to the tile's bounds during element processing.

### 9.3 Terrain Discontinuities Between Generations

**Risk:** Two adjacent generations may have different terrain elevations at their shared boundary.

**Mitigation:**
- Fetch elevation data with padding (e.g., +100 blocks beyond the bbox)
- Apply blending at boundaries using linear interpolation over 50-block transition zone
- Mark terrain stitching as experimental initially

### 9.4 Memory Pressure with Many Parallel Tiles

**Risk:** Spawning too many tile buffers exhausts RAM.

**Mitigation:**
- Process tiles in batches of `num_cpus`, not all at once
- Use stream-to-disk to free completed regions immediately
- Monitor memory and throttle if approaching limits

### 9.5 Backwards Compatibility

**Risk:** Existing users have worlds generated with the origin-at-(0,0) local projection.

**Mitigation:**
- Keep `--projection local` as an option for legacy behavior
- Existing `metadata.json` without projection info defaults to "local"
- New worlds default to Web Mercator but can be configured

---

## 10. Alternatives Considered

### 10.1 Thread-Safe Shared WorldToModify (Rejected)

Instead of per-tile buffers + merge, wrap `WorldToModify` in `Arc<Mutex<>>` or use `DashMap`.

**Why rejected:**
- Every `set_block` call would need to acquire a lock, serializing the pipeline
- DashMap provides shard-level concurrency, but the nested structure (region → chunk → section → block) would need locks at every level
- The merge approach is simpler, faster, and produces the same result

### 10.2 Processing by Minecraft Region Instead of by Tile (Rejected as Primary)

Group elements by the Minecraft region they fall into. Process each region independently.

**Why rejected as primary approach:**
- An element (building, road) can span multiple regions
- This just shifts the boundary problem to region boundaries
- However, the tile-based approach uses region-aligned tiles, achieving the same benefit

### 10.3 UTM Projection (Rejected)

Use UTM zones for the projection.

**Why rejected:**
- UTM has 60 zones; two generations in different zones cannot be placed in the same world
- Would require users to understand UTM zone numbers
- Web Mercator is simpler and globally consistent

### 10.4 BTE Conformal Dymaxion Projection (Rejected)

Use the Build The Earth projection for global coordinate consistency.

**Why rejected:**
- Extremely complex to implement (icosahedral face detection, gnomonic projection, conformal vector field interpolation, hemisphere splitting, Newton's method for inverse)
- Requires bundling ~1.5 MB of conformal adjustment data
- Produces coordinates millions of blocks from Minecraft origin, causing client rendering issues (chunk loading bugs, floating-point precision, shader artifacts)
- Forces fixed 1:1 scale, eliminating the `--scale` option users rely on
- BTE compatibility is not a project goal — users who want BTE coordinates already use BTE-specific tools
- Web Mercator achieves the same multi-generation tiling benefit with 1/10th the complexity

### 10.5 Raw Dymaxion Without BTE Modifications (Rejected)

Implement the Fuller/Dymaxion projection without BTE's conformal adjustment.

**Why rejected:**
- More complex than Web Mercator without gaining BTE interoperability
- Has visible seams at icosahedral edges
- The raw Dymaxion is neither conformal nor equal-area — shapes would be distorted

---

## Appendix: Key File Reference

| File | What Changes |
|------|-------------|
| `src/coordinate_system/transformation.rs` | Add `Projection` trait, implement WebMercator |
| `src/coordinate_system/cartesian/xzbbox/xzbbox_enum.rs` | Allow non-zero origin |
| `src/main.rs` | Add `--projection` flag, wire up new pipeline |
| `src/args.rs` | New CLI args for projection, threads |
| `src/data_processing.rs` | Tile subdivision, parallel element loop, merge |
| `src/ground_generation.rs` | Per-tile ground generation |
| `src/world_editor/mod.rs` | Per-tile editor, merge function |
| `src/world_editor/common.rs` | `WorldToModify::merge()` method |
| `src/gui.rs` | World management commands, projection selection |
| `src/gui/js/main.js` | World picker, projection UI, generation list |
| `src/gui/js/bbox.js` | Multi-generation overlays, coordinate display |
| `src/world_utils.rs` | `generations.json` reading/writing |
| *New: `src/projection/mod.rs`* | Projection trait + WebMercator implementation |
| *New: `src/projection/web_mercator.rs`* | Web Mercator forward/inverse projection |
| *New: `src/tile.rs`* | Tile subdivision + element assignment |
