# One World

One World is a second generation mode for Java Edition. Instead of a fresh
world per selected area, every run extends one persistent world. Areas
generated at different times join without gaps or seams, and the world stays
aligned to real-world coordinates.

- CLI: `arnis --one-world --output-dir <saves folder> [--world-name "My City"] --bbox ...`
- GUI: Settings > World > **One World**. The world is `Arnis One World`, or,
  with Custom World Name on, whatever the pencil picks. The pencil never
  renames a world on disk; it chooses which One World the next area goes into.
  A status line under the format toggle shows the world and its area count, and
  the map shows every generated area as an overlay.

The first run creates the world, every later run with the same name extends
it. Java and Earth only.

## How it works

### One frame for the whole world

Every area uses the same isotropic Web Mercator projection, pinned to the world:

```
k = scale * cos(origin_lat)
x = R * (lon - origin_lon) * k
z = -(merc_y(lat) - merc_y(origin_lat)) * k        (north is -z)
```

`origin_lat`, `origin_lon` and `scale` are written to the manifest
(`arnis_one_world.json`, next to `level.dat`) when the world is created, from
the centre of the first request. Block `(0, 0)` is the origin, and a lat/lon
always maps to the same block.

Web Mercator is conformal, and its axes are separable (x depends only on
longitude, z only on latitude), so a block rectangle is exactly a lat/lon
rectangle. Every fetch (OpenStreetMap, elevation, land cover, canopy) can run on
the geographic bbox of the blocks it fills, and the per-area preview PNGs sit on
the GUI map without warping. The cost is scale drift: a block is `1/scale`
metres at the origin latitude, and `cos(origin_lat) / cos(lat)` times that
elsewhere, about 2% per 100 km north or south at mid-latitudes. A run whose
area is off by more than 25% says so. The world is one continuous plane, so
Earth's curvature leaves no gaps.

Requests beyond 85 degrees latitude, or past the Minecraft world border in the
world's frame, are refused before anything is written.

`--projection web_mercator` used to be anisotropic (only x had the `cos` factor)
and was refused by `validate_args`. It is fixed and usable on its own again,
with the origin at the bbox centre.

### Chunk-aligned areas

`projection::snap_bbox_to_chunks` grows the request outward to whole chunks in
the world frame and inverse-projects the result, which the run then uses
everywhere. Every chunk belongs to exactly one generation, so an area can be
written into existing region files without merging half-chunks.

### Writing into existing region files

`RegionWriteMode::Merge` (`world_editor/java.rs`) opens `r.X.Z.mca` if it exists,
creates it empty otherwise, and writes only the chunks inside the run's
rectangle. Nothing is written outside the area; Minecraft generates the rest
from the superflat settings in `level.dat`, which match the filler plane. A
region file that cannot be read fails the run instead of being replaced, since
it holds other areas.

The bundled region template is never used here: its placeholder chunks carry
another region's positions, and Minecraft refuses to load the ones a merge
leaves standing ("chunk is in the wrong location"). Worlds created by the first
build of this feature have such chunks; they are dropped once, under the lock,
on the next run (manifest version 1 to 2).

Rewritten chunks are also removed from `entities/` and `poi/`, so entities
Minecraft already moved out of the chunk do not linger over the new blocks.

### Seams

- **Geometry**: one projection and origin, floor semantics for negative
  coordinates.
- **Ground grids**: one cell per block, sized from the block rectangle instead
  of `geo_distance`. Providers sample a bbox edge to edge, so the fetch bbox runs
  from the first to the last block centre, and rows are re-spaced from equal
  latitude steps to equal Mercator steps in place (`grid_ops`). Every block is
  sampled at its centre. Elevation, land cover and canopy are fetched with a
  margin (`ground_pad_blocks`, at least 96 blocks) and cropped, so the smoothing
  passes see the same neighbourhood on both sides of a seam.
- **Metres to Y**: the first terrain area settles
  `y = ground_level + (h_m - min_height_m) * blocks_per_metre`, keeps up to 96
  blocks of headroom below its lowest cell for lower neighbours, and stores the
  mapping in the manifest as soon as it is known, before any region is written.
  Later areas reuse it (`AffinePolicy::Fixed`).
- **Elements across a seam**: OSM ways and Overture footprints are clipped to
  the area plus 64 blocks, so a building on the edge is built whole on both
  sides; writes outside the area are dropped.
- **Climate and biomes** are read at the world origin, so neighbouring areas
  never land on different sides of a Köppen boundary or biome band.
- **Determinism**: the two unseeded RNG sites (recycling barrel loot, item frame
  side) are seeded now, and the OSM tile archive emits nodes in id order instead
  of `HashMap` order, which changed which of two overlapping trees won a block
  from run to run. Identical runs now produce identical blocks.
- **Map ids**: signage decal maps continue after the world's last map id; the
  world map item and branding map are placed for the first area only.

### Overlap

Chunks inside a new area are always generated again, also where an older area
or Minecraft itself already made them. That keeps every chunk the product of one
run and lets a newer Arnis rebuild an old area. Because it also replaces
anything a player built there, the count of existing chunks is read from the
region file headers before the run: the CLI prints it and the GUI asks for
confirmation. Areas the new one fully covers are dropped from the manifest along
with their previews.

### What the manifest fixes

Refused with a message naming the mismatch: world scale, ground level, terrain
on/off, extended build height. Taken from the manifest: the elevation source.
Forced: rotation 0, Web Mercator, no Voxy LOD cache, Mapillary facades as
blocks, no preset facades, map preview on, no Luanti. The GUI greys and pins these rows and
puts the user's own values back when One World is turned off. Everything else
may differ per area.

The extended build height pack is installed only when the world is created, so
a later Arnis cannot resize an existing world's dimension.

### Build height

Without the pack the band is vanilla Y -64 to 319, with it Y -2032 to 2031,
fixed by the first area. A later area outside the band is flattened where it
clamps, and the run says so when that affects more than 0.5% of it. 384 blocks
cover the relief of almost any city; only a world that mixes lowland with high
mountains needs the pack.

### Locking

`prepare` takes the world's `session.lock` before it reads the manifest and
holds it until the world is written, in the GUI and the CLI. A world Minecraft
has open is refused. Java locks the file with `fcntl`, so on Unix Arnis does the
same (on macOS a second, `flock`-style lock from the same process would block
it); on Windows it uses `LockFileEx`. Probing a lock this process already holds
is skipped, because closing any handle to the file would drop a POSIX lock.

### Failure handling

A world created by a run that fails is removed again, by the GUI and the CLI.
A failed run reports the error in the GUI instead of "Done!". The area is recorded only
after its regions and preview are written; the elevation mapping is stored
earlier, so a rerun of a failed first area stays on the same mapping.

Moving the spawn with a marker also moves the player into the overworld, in case
they logged out in another dimension.

Every run moves `LastPlayed` in `level.dat` to now, so the world is listed first
in Minecraft's world list.

### Manifest (`arnis_one_world.json`)

```json
{
  "version": 2,
  "created_with": "arnis 3.2.0",
  "created_at": 1789000000,
  "origin_lat": 48.1372, "origin_lon": 11.5755,
  "scale": 1.0, "ground_level": -62,
  "terrain": true, "disable_height_limit": false, "aws_only_elevation": false,
  "elevation": { "min_height_m": 421.3, "blocks_per_meter": 1.0, "ground_level": -62 },
  "next_area_id": 3,
  "areas": [
    { "id": 1, "generated_at": 1789000000, "arnis_version": "3.2.0",
      "min_x": -800, "min_z": -560, "max_x": 799, "max_z": 559,
      "min_lat": 48.132, "min_lon": 11.564, "max_lat": 48.142, "max_lon": 11.587,
      "preview": "arnis_one_world/previews/area-1.png" }
  ]
}
```

Manifests are validated on load. Preview paths are only followed inside
`arnis_one_world/previews`, since a manifest can come with a downloaded world.
`metadata.json` describes the union of all areas.

## Known limitations

1. **Facade photo panels and preset facades** write a resource pack per run
   that would replace the previous one, so they are off. Fix: merge each run's
   textures and models into the existing `resources.zip`.
2. **Voxy LOD cache** is rebuilt from one run's regions, so it is off. Fix:
   add sections to the existing database, or rebuild from all regions.
3. **World map item** shows the first area only. Fix: re-render it from all
   previews after each run.
4. **Elevation providers**: next to the edge of a regional high-resolution
   dataset, a seam carries the difference between the two datasets. Fix: pin the
   provider chain in the manifest.
5. **Water depth tiers** are computed per water body inside the area, so a lake
   cut by an area edge can get a small underwater step. Fix: compute tiers on
   the padded grid before cropping.
6. **Very large areas** whose padded grid would exceed the elevation grid cap
   (about 268 km² at scale 1) are fetched without the margin and without
   block-centre alignment.
7. **Longitude is not wrapped**: an area more than 180 degrees of longitude from
   the origin is placed the long way round the globe, or refused at the world
   border.
8. **Region palettes** can list the same blocks in a different order between
   identical runs (pre-existing, from how property blocks are deduplicated). The
   blocks themselves are identical.

## Where to look

- `src/one_world.rs`: manifest, `prepare`, `record_area`, `existing_chunks`.
- `src/projection/`: `WebMercatorProjection`, `ProjectionSpec`,
  `snap_bbox_to_chunks`.
- `src/ground.rs` (`GroundFrame`), `src/grid_ops.rs`,
  `src/elevation/postprocess.rs` (`ElevationAffine`, `AffinePolicy`).
- `src/world_editor/java.rs`: `RegionWriteMode`, merge and repair helpers.
- `src/world_utils.rs`: `SessionLock`, `world_is_locked`.
- `src/gui.rs`: `gui_one_world_info`, `gui_one_world_overlap`,
  `gui_get_one_world_overlays`; `src/gui/js/main.js` (`refreshOneWorldState`,
  `prepareOneWorldRun`); `src/gui/js/bbox.js` (`oneWorldOverlays`).

## Validation

- Ordinary generation (One World off) produces the same block hash as `main`
  in the default, rotated, flat and terrain-only modes, once `main`'s own
  node-order nondeterminism is fixed on both sides.
- Three CLI runs into one world on the Arnis test area (the area, its eastern
  neighbour, a smaller area across their seam): no chunk missing, no misplaced
  chunk, the overlap reported and rebuilt. Surface heights across the area seam
  match 86.6% exactly and 6.9% within one block, in line with ordinary chunk
  boundaries inside an area.
- The same area generated into two fresh worlds gives identical blocks.
- A second process holding `session.lock` makes the run refuse without touching
  the manifest.
- A copy of a world from the first build is repaired (7607 stray chunks
  dropped) and extended correctly.
- Unit tests cover the projection, snapping, manifest life cycle, locks, region
  merge and repair, row remap and crop, ground fetch plan and affine policies.
- The Unix lock code was compiled for macOS in isolation; it has not run on
  macOS or Linux yet. The GUI flow was reviewed but not clicked through.
