# Parallel Region Processing Architecture

## Executive Summary

This document outlines a comprehensive plan to parallelize the Arnis world generation pipeline by splitting large user-selected areas into smaller processing units (**1 Minecraft region = 512×512 blocks per unit**). The goal is to:

1. **Reduce memory usage by ~90%** by processing and flushing regions incrementally
2. **Utilize multiple CPU cores** for parallel generation
3. **Maintain visual consistency** across region boundaries (colors, elevation, etc.)

---

## Current Architecture Analysis

### Processing Pipeline Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           CURRENT PROCESSING FLOW                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  [1/7] Fetch Data (retrieve_data.rs)                                        │
│     └── Downloads OSM data for entire bbox from Overpass API                 │
│     └── Single HTTP request with full bounding box                          │
│                                                                              │
│  [2/7] Parse Data (osm_parser.rs)                                           │
│     └── Transforms lat/lon → Minecraft X/Z coordinates                      │
│     └── Clips ways/relations to bounding box (clipping.rs)                  │
│     └── Sorts elements by priority                                          │
│                                                                              │
│  [3/7] Fetch Elevation (ground.rs / elevation_data.rs)                      │
│     └── Downloads Terrarium tiles for entire bbox                           │
│     └── Builds height grid matching world dimensions                        │
│                                                                              │
│  [4/7] Process Data (data_processing.rs)                                    │
│     └── Pre-computes flood fills in parallel (floodfill_cache.rs)          │
│     └── Builds highway connectivity map                                     │
│     └── Collects building footprints                                        │
│                                                                              │
│  [5/7] Process Terrain + Elements (data_processing.rs)                      │
│     └── Iterates ALL elements sequentially                                  │
│     └── Calls element_processing/* for each element type                    │
│     └── Places blocks via WorldEditor                                       │
│                                                                              │
│  [6/7] Generate Ground (data_processing.rs)                                 │
│     └── Iterates ALL blocks in bbox                                         │
│     └── Sets grass, dirt, stone, bedrock layers                            │
│                                                                              │
│  [7/7] Save World (world_editor/mod.rs → java.rs)                          │
│     └── Iterates ALL regions in memory                                      │
│     └── Writes .mca files in parallel                                       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Key Data Structures and Memory Usage

#### WorldToModify (world_editor/common.rs)

```rust
pub struct WorldToModify {
    pub regions: FnvHashMap<(i32, i32), RegionToModify>,  // Key: (region_x, region_z)
}

pub struct RegionToModify {
    pub chunks: FnvHashMap<(i32, i32), ChunkToModify>,    // 32×32 chunks per region
}

pub struct ChunkToModify {
    pub sections: FnvHashMap<i8, SectionToModify>,       // 24 sections per chunk (-4 to 19)
}

pub struct SectionToModify {
    pub blocks: [Block; 4096],                            // 16×16×16 = 4096 blocks
    pub properties: FnvHashMap<usize, Value>,            // Block properties (stairs, slabs, etc.)
}
```

**Memory estimate per region:**
- Section: ~4KB (blocks) + ~variable (properties)
- Chunk: ~24 sections × 4KB = ~96KB minimum, typically ~200-500KB with properties
- Region: ~1024 chunks × 300KB = **~300MB per region**
- **For a 10×10 region area: ~30GB of memory required!**

#### Why Elements Are "Scattered"

The current design processes elements in OSM priority order (entrance → building → highway → waterway → water → barrier → other), NOT by spatial location. This means:

1. A building in region (0,0) might be followed by a highway in region (5,5)
2. Each `set_block()` call potentially accesses different regions
3. ALL regions must remain in memory until the end because any element might touch any region

---

## Proposed Architecture: Region-Based Parallel Processing

### Core Concept

Split the user-selected area into **processing units** of **1 Minecraft region each** (512×512 blocks = 32×32 chunks). Process each unit independently in parallel, then flush to disk immediately.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                      PROPOSED PARALLEL PROCESSING FLOW                       │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  GLOBAL PHASE (Run Once for Entire Area)                                    │
│  ═══════════════════════════════════════                                    │
│                                                                              │
│  [1] Fetch Elevation Data for ENTIRE bbox                                   │
│      └── Must be consistent across all units                                │
│      └── Store as shared read-only Arc<Ground>                              │
│                                                                              │
│  [2] Compute Processing Unit Grid                                           │
│      └── Divide bbox into N×N region units                                  │
│      └── Create sub-bboxes with small overlap for boundary elements         │
│                                                                              │
│  PARALLEL PHASE (Per Processing Unit)                                       │
│  ═════════════════════════════════════                                      │
│                                                                              │
│  For each processing unit (in parallel, using N-1 CPU cores):               │
│                                                                              │
│    [3] Fetch OSM Data for Unit's Sub-BBox                                   │
│        └── Separate Overpass API query per unit                             │
│        └── Include small buffer zone for boundary elements                  │
│                                                                              │
│    [4] Parse & Clip Elements to Unit Bounds                                 │
│        └── Same as current, but for smaller area                            │
│                                                                              │
│    [5] Pre-compute Flood Fills                                              │
│        └── Only for elements in this unit                                   │
│                                                                              │
│    [6] Process Elements                                                     │
│        └── Generate buildings, roads, etc.                                  │
│        └── Use deterministic RNG keyed by element ID                        │
│                                                                              │
│    [7] Generate Ground Layer                                                │
│        └── Only for this unit's blocks                                      │
│                                                                              │
│    [8] Save Regions to Disk                                                 │
│        └── Write .mca files immediately                                     │
│        └── FREE MEMORY for this unit                                        │
│                                                                              │
│  FINALIZATION PHASE                                                         │
│  ══════════════════                                                         │
│                                                                              │
│  [9] Wait for all units to complete                                         │
│  [10] Generate map preview (optional)                                       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Critical Considerations

### 1. Deterministic Randomness ✅ ALREADY IMPLEMENTED

The codebase already has `deterministic_rng.rs` which provides:

```rust
// Creates RNG seeded by element ID - same element always produces same random values
pub fn element_rng(element_id: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(element_id)
}

// For coordinate-based randomness
pub fn coord_rng(x: i32, z: i32, element_id: u64) -> ChaCha8Rng
```

**Impact on buildings crossing boundaries:**
- Building colors are chosen using `element_rng(element.id)` in buildings.rs
- Even if a building is processed in two different units, SAME element ID → SAME color
- The existing implementation already supports this use case!

**Files using deterministic RNG:**
- `element_processing/buildings.rs` - wall colors, window styles, accent blocks
- `element_processing/natural.rs` - grass/flower distribution
- `element_processing/tree.rs` - tree variations

### 2. Elevation Data Consistency ⚠️ REQUIRES CHANGES

**Current behavior:**
- Elevation is fetched once in `ground.rs` → `Ground::new_enabled()`
- Height grid dimensions match the world's XZ dimensions
- Lookup uses relative coordinates: `ground.level(XZPoint::new(x - min_x, z - min_z))`

**Problem:**
- If each unit downloads its own elevation tiles, slight differences in tile boundaries or interpolation could cause height discontinuities at unit boundaries

**Solution:**
1. **Download elevation ONCE for the entire area** before parallel processing starts
2. Pass `Arc<Ground>` (read-only) to all processing units
3. The `Ground::level()` function already uses world-relative coordinates, so no changes needed

```rust
// Proposed: Global elevation fetch before parallel processing
let global_ground = Arc::new(Ground::new_enabled(&args.bbox, args.scale, args.ground_level));

// Each processing unit receives a clone of the Arc
for unit in processing_units {
    let ground_ref = Arc::clone(&global_ground);
    // spawn task with ground_ref
}
```

### 3. Element Clipping ⚠️ REQUIRES NEW LOGIC

**Current clipping (clipping.rs):**
- Uses Sutherland-Hodgman algorithm to clip polygons to user's bbox
- Works on the OUTER boundary of the entire selected area

**New requirement:**
- Need to clip elements to each processing unit's internal boundary
- But with OVERLAP to handle elements that straddle unit boundaries

**Proposed approach:**

```
┌─────────────────────────────────────────────────────────────────┐
│                    UNIT BOUNDARY HANDLING                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Example: 4 processing units arranged in a 2×2 grid              │
│                                                                  │
│      Unit A          │       Unit B                              │
│   (regions 0,0-1,1)  │   (regions 2,0-3,1)                      │
│                      │                                           │
│   ┌──────────────────┼──────────────────┐                       │
│   │                  │                  │                       │
│   │     ████████     │                  │  ← Building straddles │
│   │     █ BLD  █─────┼──────────────────│    Unit A and B       │
│   │     ████████     │                  │                       │
│   │                  │                  │                       │
│   ├──────────────────┼──────────────────┤                       │
│   │                  │                  │                       │
│   │                  │                  │                       │
│   │                  │                  │                       │
│   │                  │                  │                       │
│   └──────────────────┴──────────────────┘                       │
│      Unit C          │       Unit D                              │
│   (regions 0,2-1,3)  │   (regions 2,2-3,3)                      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Strategy for boundary elements:**

1. **Expanded Fetch BBox**: Each unit fetches OSM data with a buffer zone (e.g., +256 blocks)
2. **Clip to Processing BBox**: Clip elements to the unit's actual processing bounds
3. **Process Normally**: Elements partially in the unit are still processed, just clipped
4. **Deterministic Results**: Same element in adjacent units produces identical blocks due to RNG seeding

**Example: Building straddling Unit A and B**

| Step | Unit A | Unit B |
|------|--------|--------|
| Fetch | Gets building (with buffer) | Gets building (with buffer) |
| Clip | Clips to Unit A bounds → left half | Clips to Unit B bounds → right half |
| Color | `element_rng(building_id)` → BLUE | `element_rng(building_id)` → BLUE |
| Place | Places left half in blue | Places right half in blue |
| **Result** | **Seamless blue building across boundary** |

### 4. OSM Data Downloading Strategy ⚠️ REQUIRES CAREFUL DESIGN

**Options:**

#### Option A: Download Once, Distribute Elements (RECOMMENDED)

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                  │
│  [1] Download ALL OSM data for entire bbox (single API call)    │
│  [2] Parse into ProcessedElements                                │
│  [3] For each processing unit:                                   │
│      └── Filter elements that intersect unit's bbox             │
│      └── Clip filtered elements to unit bounds                  │
│      └── Send to parallel processor                             │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Pros:**
- Single Overpass API call (respects rate limits)
- No duplicate data transfer
- Elements are already parsed, just need filtering

**Cons:**
- Must keep all elements in memory during distribution phase
- For very large areas, this might still be memory-intensive

#### Option B: Download Per Unit (Simpler, Higher Bandwidth)

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                  │
│  For each processing unit (sequentially or with rate limiting): │
│      [1] Download OSM data for unit's expanded bbox             │
│      [2] Parse into ProcessedElements                            │
│      [3] Send to parallel processor                             │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

**Pros:**
- Lower peak memory usage
- Simpler code structure

**Cons:**
- Multiple API calls (may hit rate limits)
- Duplicate data transfer for overlapping areas
- Slower due to network latency

#### Recommendation: Option A with Streaming

Download once, but use a streaming approach to distribute elements to units:

```rust
// Pseudo-code for element distribution
fn distribute_elements_to_units(
    elements: Vec<ProcessedElement>,
    units: &[ProcessingUnit],
) -> Vec<Vec<ProcessedElement>> {
    let mut unit_elements = vec![Vec::new(); units.len()];
    
    for element in elements {
        let element_bbox = compute_element_bbox(&element);
        for (i, unit) in units.iter().enumerate() {
            if unit.expanded_bbox.intersects(&element_bbox) {
                // Clone element for each unit that needs it
                // (or use Arc for large elements)
                unit_elements[i].push(element.clone());
            }
        }
    }
    
    unit_elements
}
```

### 5. Flood Fill Cache ⚠️ REQUIRES CHANGES

**Current behavior:**
- `FloodFillCache::precompute()` runs in parallel for ALL elements
- Results are stored in a `FnvHashMap<u64, Vec<(i32, i32)>>`
- Cache is consumed during sequential element processing

**Problem:**
- If we process units in parallel, each unit needs its own flood fill cache
- But we don't want to re-compute the same flood fills multiple times

**Solution A: Per-Unit Flood Fill (Simpler)**

```rust
// Each unit computes flood fills only for its elements
fn process_unit(unit_elements: Vec<ProcessedElement>) {
    let flood_fill_cache = FloodFillCache::precompute(&unit_elements, timeout);
    // Process elements using this cache
}
```

**Pros:** Simple, no coordination needed
**Cons:** Elements at boundaries may be flood-filled twice

**Solution B: Global Flood Fill + Distribution (More Complex)**

```rust
// Compute flood fills globally, then distribute to units
let global_cache = FloodFillCache::precompute(&all_elements, timeout);

// For each unit, create a view into the global cache
let unit_caches: Vec<_> = units.iter()
    .map(|unit| global_cache.filter_for_bbox(&unit.bbox))
    .collect();
```

**Recommendation:** Start with Solution A. The overhead of re-computing some flood fills at boundaries is acceptable given the simplicity.

### 6. Building Footprints Bitmap ⚠️ REQUIRES CHANGES

**Current behavior:**
- `BuildingFootprintBitmap` is a memory-efficient bitmap covering the entire world
- Used to prevent trees from spawning inside buildings
- Created AFTER flood fill precomputation

**Problem:**
- With parallel processing, each unit only knows about buildings in its own area
- A tree in Unit B might spawn inside a building that exists in Unit A (near boundary)

**Solution:**
- Compute building footprints GLOBALLY before parallel processing
- Use `Arc<BuildingFootprintBitmap>` shared across all units (read-only)

```rust
// Global building footprint computation
let all_building_coords = compute_all_building_footprints(&all_elements, &global_xzbbox);
let global_footprints = Arc::new(BuildingFootprintBitmap::from(all_building_coords));

// Each unit receives Arc clone
for unit in units {
    let footprints = Arc::clone(&global_footprints);
    // spawn task
}
```

### 7. Highway Connectivity ⚠️ REQUIRES CHANGES

**Current behavior:**
- `highways::build_highway_connectivity_map()` creates a map of connected highway segments
- Used for intersection detection and road marking placement

**Problem:**
- Highway segments crossing unit boundaries won't see their full connectivity

**Solution:**
- Build highway connectivity map GLOBALLY before parallel processing
- Pass as `Arc<HighwayConnectivityMap>` to all units

### 8. Water Areas and Ring Merging ✅ ALREADY SUPPORTED

**Current behavior:**
- Water relations contain multiple ways that must be merged into closed rings
- `merge_way_segments()` in water_areas.rs handles this
- **Clipping happens AFTER ring merging** via `clip_water_ring_to_bbox()`
- Water uses `inverse_floodfill()` which iterates over bounding box (not flood fill)

**Why water CAN be clipped per-unit:**
1. Ring merging happens on the UNCLIPPED ways (preserved in osm_parser.rs)
2. After merging, `clip_water_ring_to_bbox()` clips the assembled polygon
3. The `inverse_floodfill` algorithm iterates block-by-block within bounds
4. Each unit can independently clip and fill its portion of a water body

**No special handling needed** - water relations work the same as other elements:
- Distribute relation to units that intersect its bbox
- Each unit clips to its own bounds
- Each unit fills its portion independently

### 9. Element Priority Order ⚠️ MUST BE PRESERVED

**Current behavior:**
- Elements are sorted by priority before processing (osm_parser.rs):
  ```rust
  const PRIORITY_ORDER: [&str; 6] = [
      "entrance", "building", "highway", "waterway", "water", "barrier",
  ];
  ```
- This ensures entrances are placed before buildings (so doors work)
- Buildings before highways (so sidewalks don't overwrite buildings)

**Requirement:**
- Each unit must process its elements in the SAME priority order
- This is natural: just sort the unit's elements the same way

### 10. SPONGE Block as Placeholder ⚠️ MINOR CONSIDERATION

**Current behavior:**
- `SPONGE` block is used as a blacklist marker in some places
- Example: `editor.set_block(actual_block, x, 0, z, None, Some(&[SPONGE]));`
- Prevents certain blocks from overwriting sponge blocks

**Impact on parallel processing:**
- None - this is a per-block check, not cross-region coordination
- Each unit handles its own sponge blocks independently

### 11. Tree Placement and Building Footprints ⚠️ REQUIRES GLOBAL FOOTPRINTS

**Current behavior:**
- Trees check `building_footprints.contains(x, z)` before spawning
- Prevents trees from appearing inside buildings
- Uses `coord_rng(x, z, element_id)` for deterministic placement

**Problem with per-unit footprints:**
- A tree near a unit boundary might not see a building from the adjacent unit
- Could spawn a tree inside a building that exists in neighbor unit

**Solution (already planned):**
- Compute building footprints GLOBALLY before parallel processing
- Pass as `Arc<BuildingFootprintBitmap>` to all units
- Tree placement will correctly avoid all buildings

### 12. Relations with Multiple Members Across Units ⚠️ REQUIRES CAREFUL HANDLING

**Current behavior:**
- Relations (buildings, landuse, leisure, natural) process each member way
- Member ways can be scattered across the entire bbox

**Example: Building relation with courtyard**
```
Building Relation:
  - Outer way 1 (in Unit A)
  - Outer way 2 (in Unit A and B)  ← straddles boundary
  - Inner way (courtyard, in Unit A)
```

**Strategy:**
1. Distribute entire relation to all units that any member touches
2. Each unit clips all members to its bounds
3. Each unit processes the clipped relation independently
4. Deterministic RNG ensures consistent colors/styles

**Important:** The relation-level tags (e.g., `building:levels`) must be preserved for all units processing that relation.

---

## Proposed Processing Unit Structure

### ProcessingUnit Definition

```rust
struct ProcessingUnit {
    /// Which region this unit covers (1 region per unit)
    region_x: i32,
    region_z: i32,
    
    /// Minecraft coordinate bounds for this unit (512×512 blocks)
    min_x: i32,  // region_x * 512
    max_x: i32,  // region_x * 512 + 511
    min_z: i32,  // region_z * 512
    max_z: i32,  // region_z * 512 + 511
    
    /// Expanded bounds for element fetching (includes buffer for boundary elements)
    fetch_min_x: i32,
    fetch_max_x: i32,
    fetch_min_z: i32,
    fetch_max_z: i32,
}
```

### Unit Grid Calculation

```rust
fn compute_processing_units(
    global_xzbbox: &XZBBox,
    buffer_blocks: i32,     // e.g., 64-128 blocks overlap
) -> Vec<ProcessingUnit> {
    let blocks_per_region = 512;  // 32 chunks × 16 blocks
    
    // Calculate which regions are covered by the bbox
    let min_region_x = global_xzbbox.min_x() >> 9;  // divide by 512
    let max_region_x = global_xzbbox.max_x() >> 9;
    let min_region_z = global_xzbbox.min_z() >> 9;
    let max_region_z = global_xzbbox.max_z() >> 9;
    
    let mut units = Vec::new();
    
    // Create one unit per region
    for rx in min_region_x..=max_region_x {
        for rz in min_region_z..=max_region_z {
            // Compute Minecraft coordinate bounds for this region
            let min_x = rx * blocks_per_region;
            let max_x = min_x + blocks_per_region - 1;
            let min_z = rz * blocks_per_region;
            let max_z = min_z + blocks_per_region - 1;
            
            // Add buffer for fetch bounds (clamped to global bbox)
            let fetch_min_x = (min_x - buffer_blocks).max(global_xzbbox.min_x());
            let fetch_max_x = (max_x + buffer_blocks).min(global_xzbbox.max_x());
            let fetch_min_z = (min_z - buffer_blocks).max(global_xzbbox.min_z());
            let fetch_max_z = (max_z + buffer_blocks).min(global_xzbbox.max_z());
            
            units.push(ProcessingUnit {
                region_x: rx,
                region_z: rz,
                min_x, max_x, min_z, max_z,
                fetch_min_x, fetch_max_x, fetch_min_z, fetch_max_z,
            });
        }
    }
    
    units
}
```

### Parallel Execution Strategy

```rust
fn process_units_parallel(
    units: Vec<ProcessingUnit>,
    elements: &[ProcessedElement],
    global_ground: Arc<Ground>,
    global_building_footprints: Arc<BuildingFootprintBitmap>,
    global_highway_connectivity: Arc<HighwayConnectivityMap>,
    args: &Args,
) {
    // Use CPU-1 cores for parallel processing
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1);
    
    units.into_par_iter()
        .with_min_len(1)  // Process 1 unit per task
        .for_each(|unit| {
            // 1. Filter elements that intersect this unit's fetch bounds
            let unit_elements = filter_elements_for_unit(elements, &unit);
            
            // 2. Clip elements to unit's actual bounds
            let clipped_elements = clip_elements_to_unit(unit_elements, &unit);
            
            // 3. Create per-unit structures
            let unit_xzbbox = XZBBox::new(unit.min_x, unit.max_x, unit.min_z, unit.max_z);
            let mut editor = WorldEditor::new(args.path.clone(), &unit_xzbbox, ...);
            editor.set_ground(Arc::clone(&global_ground));
            
            // 4. Pre-compute flood fills for this unit's elements
            let flood_fill_cache = FloodFillCache::precompute(&clipped_elements, args.timeout.as_ref());
            
            // 5. Process elements (same as current, just for this unit)
            for element in clipped_elements {
                process_element(&mut editor, &element, ...);
            }
            
            // 6. Generate ground layer for this unit
            generate_ground_for_unit(&mut editor, &unit, &global_ground);
            
            // 7. Save region immediately and FREE MEMORY
            editor.save_single_region(unit.region_x, unit.region_z);
            drop(editor);  // Release memory
        });
}
```

---

## Memory Usage Comparison

### Understanding Minecraft Region Sizes

```
1 Region = 32×32 chunks = 512×512 blocks (horizontally)
1 Chunk  = 16×16×384 blocks (with sections from Y=-64 to Y=319)
```

### Current Architecture (All Regions in Memory)

| Stage | Memory Usage |
|-------|--------------|
| OSM Data (parsed) | ~50-200 MB |
| Flood Fill Cache | ~100-500 MB |
| Building Footprints | ~10-50 MB |
| WorldToModify (all regions) | **~300 MB × N regions** |
| **Total for 100 regions** | **~30+ GB** |

### Unit Size Analysis

The optimal unit size depends on balancing:
1. **Memory per unit** - Larger units = more memory
2. **Parallelism overhead** - Smaller units = more coordination
3. **Boundary overhead** - More units = more elements processed multiple times

| Unit Size | Blocks | Memory per Unit | Parallel Units (8 cores) | Peak Memory |
|-----------|--------|-----------------|--------------------------|-------------|
| 1 region (32×32 chunks) | 512×512 | ~300 MB | 7 units | ~2.5 GB |
| 2×2 regions | 1024×1024 | ~1.2 GB | 7 units | ~9 GB |
| 4×4 regions | 2048×2048 | ~4.8 GB | 7 units | ~35 GB |

### Recommendation: 1 Region Per Unit

**1 region per unit is optimal because:**

1. **Lowest memory footprint** - Only ~300 MB per unit
2. **Natural alignment** - Regions are the atomic save unit in Minecraft (.mca files)
3. **Maximum parallelism** - More units = better CPU utilization
4. **Simple boundary logic** - No partial region handling

**Memory calculation for 7 parallel units (8-core CPU, using 7):**
- Per-unit WorldToModify: ~300 MB
- Per-unit flood fill cache: ~50 MB
- Per-unit OSM elements: ~20 MB
- **Peak memory: ~370 MB × 7 = ~2.6 GB**

Plus global shared data:
- Elevation data: ~50-100 MB
- Building footprints: ~10-50 MB
- Highway connectivity: ~20-50 MB

**Total peak: ~3 GB** (vs ~30 GB for 100 regions currently!)

### Why Not Smaller Than 1 Region?

- Regions are the minimum save unit for Minecraft
- Going smaller would require buffering partial regions
- No memory benefit (still need full region in memory to save)

---

## Implementation Phases

### Phase 1: Refactor Global Data Preparation

**Goal:** Extract global computations that must run before parallel processing

**Changes:**
1. Move elevation fetching to a separate global phase
2. Move building footprint collection to global phase
3. Move highway connectivity map building to global phase
4. Create shared data structures with `Arc<T>`

**Files affected:**
- `data_processing.rs` - restructure `generate_world_with_options()`
- `ground.rs` - no changes, already returns `Ground`
- `floodfill_cache.rs` - add method to collect building footprints globally
- `element_processing/highways.rs` - extract connectivity map building

### Phase 2: Implement Processing Unit Grid

**Goal:** Add logic to divide the world into processing units

**Changes:**
1. Create `processing_unit.rs` module
2. Implement grid computation
3. Implement element-to-unit distribution
4. Add unit-level bounding box clipping

**New files:**
- `src/processing_unit.rs`

### Phase 3: Parallelize Unit Processing

**Goal:** Process units in parallel using rayon

**Changes:**
1. Create per-unit WorldEditor instances
2. Implement unit processing function
3. Add parallel execution with CPU cap
4. Implement region saving after unit completion

**Files affected:**
- `data_processing.rs` - main parallel loop
- `world_editor/mod.rs` - support per-unit saving

### Phase 4: Handle Boundary Cases

**Goal:** Ensure seamless results across unit boundaries

**Changes:**
1. Verify deterministic RNG produces identical results
2. Implement special handling for large water bodies
3. Add boundary verification tests
4. Optimize overlap buffer size

**Files affected:**
- `element_processing/water_areas.rs` - global water handling
- `clipping.rs` - potential optimizations

### Phase 5: Optimize Memory Management

**Goal:** Fine-tune memory usage and parallelism

**Changes:**
1. Implement memory pressure monitoring
2. Add dynamic unit size adjustment
3. Optimize flood fill cache memory
4. Profile and optimize hot paths

---

## Testing Strategy

### Unit Tests

1. **Deterministic RNG Test**
   - Process same building in two units
   - Verify identical colors/styles

2. **Elevation Consistency Test**
   - Check ground level at unit boundaries
   - Verify no height discontinuities

3. **Clipping Accuracy Test**
   - Verify elements clipped correctly at unit boundaries
   - Check polygon integrity after clipping

### Integration Tests

1. **Small Area Test**
   - Process 2×2 region area
   - Verify world loads correctly in Minecraft

2. **Boundary Building Test**
   - Create world with buildings at unit boundaries
   - Verify buildings are complete and correctly colored

3. **Large Water Body Test**
   - Process area with lake spanning multiple units
   - Verify water body is continuous

### Performance Tests

1. **Memory Usage Test**
   - Monitor peak memory during processing
   - Compare with current architecture

2. **CPU Utilization Test**
   - Verify parallel units use expected cores
   - Measure speedup vs sequential processing

---

## Configuration Options

### Proposed CLI Arguments

```rust
/// Number of CPU cores to use for parallel processing (default: available - 1)
/// Set to 1 to disable parallel processing
#[arg(long, default_value_t = 0)]
pub parallel_cores: usize,

/// Buffer size for boundary overlap in blocks (default: 64)
/// Larger values ensure buildings at boundaries are complete but increase processing time
#[arg(long, default_value_t = 64)]
pub boundary_buffer: i32,
```

---

## Risk Assessment

### High Risk

| Risk | Mitigation |
|------|------------|
| Elevation discontinuities at boundaries | Use global elevation data (already planned) |
| Race conditions in file writing | Each unit writes different regions (no overlap) |
| Trees spawning inside buildings at boundaries | Use global building footprints bitmap |

### Medium Risk

| Risk | Mitigation |
|------|------------|
| Overpass API rate limiting | Download once globally, distribute elements |
| Complex relations broken at boundaries | Distribute full relation to all touching units |
| Highway connectivity missing at boundaries | Build connectivity map globally |

### Low Risk

| Risk | Mitigation |
|------|------------|
| Different random values at boundaries | Deterministic RNG already implemented |
| Performance regression | Benchmark before/after, make parallel optional |
| Water bodies split incorrectly | Water already supports clipping via `clip_water_ring_to_bbox` |

---

## Questions to Resolve

1. **Should we support Bedrock format with this change?**
   - Bedrock writes to a single .mcworld file (LevelDB database)
   - May need different handling (write to temp, merge at end)
   - Could be deferred to a follow-up implementation

2. **What buffer size for boundary overlap?**
   - Current thinking: 64-128 blocks should be sufficient
   - Most buildings are smaller than this
   - Larger buffers = more duplicate processing

3. **Should flood fills be computed globally or per-unit?**
   - Per-unit is simpler and avoids coordination
   - Some redundant computation at boundaries (acceptable)
   - **Recommendation:** Start per-unit

4. **How to report progress across parallel units?**
   - Current progress is linear (element by element)
   - With parallel, need aggregated progress reporting
   - Option: Track completed regions, report as percentage

5. **Should we limit parallelism based on available RAM?**
   - Could detect system RAM and adjust parallel units
   - Or just document memory requirements per parallel unit
   - **Recommendation:** Start with CPU-1 cores, let users override

---

## Summary

The proposed parallel region processing architecture will:

1. ✅ **Reduce memory usage by ~90%** by processing 1 region at a time per unit (~300 MB vs ~30 GB for 100 regions)
2. ✅ **Utilize multiple CPU cores** through rayon-based parallel processing (CPU-1 cores)
3. ✅ **Maintain visual consistency** using deterministic RNG and global shared data
4. ✅ **Be backward compatible** with a `--no-parallel` flag for the current behavior

The main implementation work is:
- Refactoring to extract global computations (elevation, building footprints, highway connectivity)
- Adding element-to-unit distribution logic with proper clipping
- Per-unit WorldEditor instances with immediate region saving

**The design is simpler than originally thought** because:
- Water relations already support clipping (no special handling)
- Deterministic RNG already exists (no changes needed)
- Priority order is preserved naturally (just sort per-unit)

Estimated implementation effort: **3-4 weeks** for a fully tested solution.
