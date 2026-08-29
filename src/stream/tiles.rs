//! Tile cache and tile generation for stream mode.
//!
//! Stream mode never generates a single chunk on its own: the OSM pipeline needs a whole area to
//! make sense of ways, buildings and terrain, so the unit of work here is a **tile** — a square
//! block of the world, generated once, kept in RAM, and served out 16x16 chunks at a time.
//!
//! One tile job is:
//!
//! 1. take the tile's block rect and grow it by a margin on all four sides,
//! 2. inverse-project that padded rect through the anchor's pinned transverse Mercator projection
//!    to get a lat/lon bbox,
//! 3. fetch OSM + elevation for the padded bbox and run the ordinary generation pipeline in memory,
//! 4. throw the margin away and encode the chunks that fall inside the strict tile rect.
//!
//! The margin exists because OSM geometry does not respect tile boundaries: a road, a river or a
//! building that straddles the edge must be generated with its whole shape in view, otherwise the
//! two tiles either side of it disagree about what is there. Generating the margin and discarding
//! it is what makes neighbouring tiles agree.
//!
//! Everything the pipeline sees is in ONE absolute coordinate frame — the anchor's — which is why
//! the parse goes through [`crate::osm_parser::parse_osm_data_pinned`] rather than the ordinary
//! entry point that derives its origin from the bbox midpoint.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fnv::FnvHashMap;

use crate::args::Args;
use crate::block_definitions::{Block, AIR};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::data_processing::{self, GenerationOptions, GenerationSink};
use crate::elevation::AbsoluteVerticalMapping;
use crate::ground::Ground;
use crate::osm_parser::{self, OsmData};
use crate::projection::{Projection, TransverseMercatorProjection};
use crate::retrieve_data;
use crate::stream::projection::Anchor;
use crate::stream::protocol::{
    blockstate_string, BlockEntityPayload, ChunkPayload, SectionPayload,
};
use crate::world_editor::{
    BlockStorage, ChunkToModify, SectionToModify, WorldFormat, WorldToModify,
};

use fastnbt::Value;

/// Blocks along one edge of a Minecraft chunk.
const CHUNK_BLOCKS: i32 = 16;

/// Cells in one 16x16x16 section.
const SECTION_CELLS: usize = 4096;

/// Side length of a tile, in blocks.
///
/// 512 is the pipeline's own natural grain: it is exactly one Anvil region (32x32 chunks), which
/// is the granularity the generator already bands and flushes work at, so a tile job maps onto
/// one unit of work the rest of the codebase is tuned for. It is also large enough that the
/// fixed per-job cost (two HTTP fetches, the flood-fill caches, the elevation grid) is amortised
/// over 1024 chunks rather than paid per chunk.
pub const DEFAULT_TILE_SIZE: i32 = 512;

/// Blocks of context generated around a tile and then discarded.
///
/// OSM ways and buildings cross tile boundaries, and the generator's shape-level passes
/// (flood fill, building footprints, road masks, water areas) need the whole element in view to
/// produce the same blocks on both sides of the seam. 128 blocks covers the overwhelming majority
/// of individual elements; anything longer (a motorway, a river) is generated as a clipped
/// linear feature whose local appearance does not depend on the far end.
pub const DEFAULT_MARGIN: i32 = 128;

/// Tiles kept resident in the LRU cache.
///
/// A 512-block tile of dense city is tens of MB once encoded, so 16 tiles is a few hundred MB in
/// the worst case — enough to hold a player's immediate surroundings plus the direction they are
/// walking, without competing with the generator itself for RAM.
pub const DEFAULT_CACHE_TILES: usize = 16;

/// Read an `i32` tuning knob from the environment, falling back to `default`.
///
/// Follows the existing `ARNIS_*` convention (see `should_stream_to_disk` in
/// `src/data_processing.rs`): unset or unparseable means "use the default", silently.
fn env_i32(name: &str, default: i32) -> i32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
        .unwrap_or(default)
}

/// Effective tile size in blocks, honouring `ARNIS_STREAM_TILE_SIZE`.
///
/// Clamped to a positive multiple of 16 so tiles always break on chunk boundaries; a tile edge
/// that cut through a chunk would make that chunk belong to two tiles at once.
pub fn tile_size() -> i32 {
    let raw = env_i32("ARNIS_STREAM_TILE_SIZE", DEFAULT_TILE_SIZE);
    if raw < CHUNK_BLOCKS {
        return CHUNK_BLOCKS;
    }
    raw - raw.rem_euclid(CHUNK_BLOCKS)
}

/// Effective margin in blocks, honouring `ARNIS_STREAM_MARGIN`. Never negative.
pub fn margin() -> i32 {
    env_i32("ARNIS_STREAM_MARGIN", DEFAULT_MARGIN).max(0)
}

/// Effective cache capacity in tiles, honouring `ARNIS_STREAM_CACHE_TILES`. Never zero.
pub fn cache_tiles() -> usize {
    let raw = env_i32("ARNIS_STREAM_CACHE_TILES", DEFAULT_CACHE_TILES as i32);
    raw.max(1) as usize
}

// ---------------------------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------------------------

/// Identifies one generated tile: which anchor's frame it lives in, and where in that frame.
///
/// Tile `(tx, tz)` covers blocks `[tx * tile_size, (tx + 1) * tile_size)` on each axis. Tile
/// indices are derived with `div_euclid`, so the tile containing block -1 is tile -1 (not tile 0),
/// which is the only behaviour that keeps the tiling continuous across the origin. Negative
/// coordinates are the normal case here — an anchor pins a real place to an arbitrary world
/// position and the world grows in every direction from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// The anchor whose projection defines this tile's coordinate frame.
    pub anchor_id: u32,
    /// Tile index along X.
    pub tx: i32,
    /// Tile index along Z.
    pub tz: i32,
}

impl TileKey {
    /// The tile containing block `(x, z)` in `anchor_id`'s frame.
    pub fn from_block(anchor_id: u32, x: i32, z: i32, tile_size: i32) -> Self {
        Self {
            anchor_id,
            tx: tile_of_block(x, tile_size),
            tz: tile_of_block(z, tile_size),
        }
    }

    /// The tile containing chunk `(cx, cz)` in `anchor_id`'s frame.
    pub fn from_chunk(anchor_id: u32, cx: i32, cz: i32, tile_size: i32) -> Self {
        Self {
            anchor_id,
            tx: tile_of_chunk(cx, tile_size),
            tz: tile_of_chunk(cz, tile_size),
        }
    }

    /// The half-open block rect this tile covers.
    pub fn rect(&self, tile_size: i32) -> BlockRect {
        BlockRect {
            min_x: self.tx * tile_size,
            min_z: self.tz * tile_size,
            max_x: (self.tx + 1) * tile_size,
            max_z: (self.tz + 1) * tile_size,
        }
    }
}

/// Tile index containing block coordinate `b`.
///
/// `div_euclid`, never `/`: integer division truncates towards zero, which would fold blocks -1
/// and 0 into the same tile and shift every tile west of the origin by one.
#[inline]
pub fn tile_of_block(b: i32, tile_size: i32) -> i32 {
    b.div_euclid(tile_size)
}

/// Tile index containing chunk coordinate `c`.
///
/// Saturating, not wrapping: `overflow-checks` is on in release, so a plain multiply here
/// panics on whichever thread a client-supplied coordinate reaches. Requests are range-checked
/// before they get this far (`session::MAX_CHUNK_COORD`); this keeps the primitive itself total
/// so no future caller can turn a bad number into a panic.
#[inline]
pub fn tile_of_chunk(c: i32, tile_size: i32) -> i32 {
    tile_of_block(c.saturating_mul(CHUNK_BLOCKS), tile_size)
}

/// Chunk coordinate containing block coordinate `b`.
#[inline]
#[allow(dead_code)]
pub fn chunk_of_block(b: i32) -> i32 {
    b.div_euclid(CHUNK_BLOCKS)
}

/// Lowest block coordinate covered by chunk `c`.
///
/// Saturating for the same reason as [`tile_of_chunk`]: `c * 16` overflows for `|c| >= 2^27`,
/// and with `overflow-checks` on that is a panic on the connection thread rather than an error
/// the client can be told about.
#[inline]
#[allow(dead_code)]
pub fn chunk_min_block(c: i32) -> i32 {
    c.saturating_mul(CHUNK_BLOCKS)
}

/// A half-open rectangle of blocks: `[min_x, max_x) x [min_z, max_z)`.
///
/// Half-open because tiling is: tile `t` ends exactly where tile `t + 1` begins, with no block
/// belonging to both and none falling between them. (`XZBBox`, which the pipeline uses, is
/// inclusive on both ends — [`BlockRect::to_xzbbox`] does that conversion in one place.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRect {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl BlockRect {
    /// Width in blocks.
    #[inline]
    #[allow(dead_code)]
    pub fn width(&self) -> i32 {
        self.max_x - self.min_x
    }

    /// Depth in blocks.
    #[inline]
    #[allow(dead_code)]
    pub fn depth(&self) -> i32 {
        self.max_z - self.min_z
    }

    /// This rect grown by `m` blocks on all four sides.
    #[inline]
    pub fn expand(&self, m: i32) -> Self {
        Self {
            min_x: self.min_x - m,
            min_z: self.min_z - m,
            max_x: self.max_x + m,
            max_z: self.max_z + m,
        }
    }

    /// Whether block `(x, z)` falls inside.
    #[inline]
    #[allow(dead_code)]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x < self.max_x && z >= self.min_z && z < self.max_z
    }

    /// Whether the whole 16x16 footprint of chunk `(cx, cz)` falls inside.
    #[inline]
    pub fn contains_chunk(&self, cx: i32, cz: i32) -> bool {
        let x0 = chunk_min_block(cx);
        let z0 = chunk_min_block(cz);
        x0 >= self.min_x
            && z0 >= self.min_z
            && x0 + CHUNK_BLOCKS <= self.max_x
            && z0 + CHUNK_BLOCKS <= self.max_z
    }

    /// The equivalent inclusive `XZBBox` the generation pipeline expects.
    #[allow(dead_code)]
    pub fn to_xzbbox(self) -> Result<XZBBox, String> {
        XZBBox::rect_from_min_max(self.min_x, self.min_z, self.max_x - 1, self.max_z - 1)
    }
}

// ---------------------------------------------------------------------------------------------
// Job configuration
// ---------------------------------------------------------------------------------------------

/// The identity of one anchor, reduced to the numbers that change what a tile looks like.
///
/// Taken from the anchor's pinned projection rather than from the anchor struct, because that is
/// the thing generation actually consumes: origin lat/lon, scale, and the world position the
/// origin is nailed to. Two anchors with the same digest produce byte-identical tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnchorDigest {
    pub id: u32,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
    pub mc_x: f64,
    pub mc_z: f64,
}

impl AnchorDigest {
    /// Digest of `anchor`, registered under `id`.
    pub fn of(id: u32, anchor: &Anchor) -> Self {
        let p = anchor.projection();
        Self {
            id,
            origin_lat: p.origin_lat,
            origin_lon: p.origin_lon,
            scale: p.scale,
            mc_x: p.false_easting,
            mc_z: p.false_northing,
        }
    }

    fn hash_into(&self, h: &mut impl Hasher) {
        self.id.hash(h);
        self.origin_lat.to_bits().hash(h);
        self.origin_lon.to_bits().hash(h);
        self.scale.to_bits().hash(h);
        self.mc_x.to_bits().hash(h);
        self.mc_z.to_bits().hash(h);
    }
}

/// Everything a tile job needs that is not the tile itself.
///
/// Built once per session from the client's `Hello` and shared by every job on that session.
/// `Args` is neither `Clone` nor cheap, so it is shared behind an `Arc`.
///
/// **`args.debug` must be `false`.** `generate_world_in_memory` documents this: the debug PNG
/// dumps run during `Ground` construction and write to bare relative filenames.
pub struct TileJobConfig {
    /// The generation settings, exactly as the ordinary pipeline consumes them.
    pub args: Arc<Args>,
    /// Absolute metre->Y mapping, so terrain lines up across tiles instead of being normalised
    /// per fetch.
    pub vertical: AbsoluteVerticalMapping,
    /// World floor the client declared (`VerticalMapping.minY`). A multiple of 16.
    pub world_min_y: i32,
    /// World height the client declared (`VerticalMapping.height`). A multiple of 16.
    pub world_height: i32,
    /// When set, tile fetches read this file instead of querying Overpass.
    pub local_osm_file: Option<String>,
    /// Tile side length in blocks.
    pub tile_size: i32,
    /// Margin generated around each tile and then discarded.
    pub margin: i32,
    /// Every anchor registered on the session. Only used by [`config_hash`]: moving or replacing
    /// an anchor changes the world, and the cache has to notice.
    pub anchors: Vec<AnchorDigest>,
}

impl TileJobConfig {
    /// Highest legal Y in the client's declared dimension.
    #[inline]
    pub fn world_max_y(&self) -> i32 {
        self.world_min_y + self.world_height - 1
    }
}

/// Hash of everything that would change the blocks a tile contains.
///
/// Stored alongside the cache; when it changes, every cached tile is stale and the cache is
/// dropped wholesale. `env!("ARNIS_BUILD_HASH")` is in here so a rebuilt binary never serves
/// tiles generated by the previous one, and `CARGO_PKG_VERSION` so a released version bump does
/// the same.
///
/// `f64` fields are hashed via `to_bits`: `Args` is not `Hash` (and `f64` cannot be), so every
/// float in here is folded in as its exact bit pattern.
pub fn config_hash(cfg: &TileJobConfig) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();

    env!("CARGO_PKG_VERSION").hash(&mut h);
    env!("ARNIS_BUILD_HASH").hash(&mut h);

    // Generation config (the wire `GenConfig`, plus the few Args fields that change output).
    let a = &cfg.args;
    a.scale.to_bits().hash(&mut h);
    a.fillground.hash(&mut h);
    a.interior.hash(&mut h);
    a.use_3d.hash(&mut h);
    a.overture.hash(&mut h);
    a.canopy_height.hash(&mut h);
    a.terrain().hash(&mut h);
    a.skip_objects().hash(&mut h);
    a.ground_level.hash(&mut h);
    a.disable_height_limit.hash(&mut h);
    a.legacy_trees.hash(&mut h);
    a.bake_lighting.hash(&mut h);
    a.rotation.to_bits().hash(&mut h);
    cfg.local_osm_file.hash(&mut h);

    // Vertical mapping.
    cfg.vertical.sea_level_y.hash(&mut h);
    cfg.vertical.blocks_per_meter.to_bits().hash(&mut h);
    cfg.world_min_y.hash(&mut h);
    cfg.world_height.hash(&mut h);

    // Tiling.
    cfg.tile_size.hash(&mut h);
    cfg.margin.hash(&mut h);

    // Anchors: position and pinning, in registration order.
    cfg.anchors.len().hash(&mut h);
    for anchor in &cfg.anchors {
        anchor.hash_into(&mut h);
    }

    h.finish()
}

// ---------------------------------------------------------------------------------------------
// Generated tiles
// ---------------------------------------------------------------------------------------------

/// One finished tile: its chunks, keyed by ABSOLUTE chunk coordinates.
///
/// Absolute, not tile-relative, so serving a chunk is a single map lookup with the coordinates
/// the client already asked for. A chunk absent from the map is all air — stream mode never
/// synthesizes the grass filler chunk the disk writer invents for empty regions, because the mod
/// is placing these into an existing world and a filler plane would bulldoze it.
pub struct GeneratedTile {
    /// Chunks inside the strict tile rect, keyed by `(chunk_x, chunk_z)`.
    pub chunks: FnvHashMap<(i32, i32), ChunkPayload>,
    /// True when the elevation layer clamped terrain against the world's Y bounds.
    pub clipped: bool,
}

// ---------------------------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------------------------

/// Bail out between pipeline stages if the client cancelled.
fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("cancelled".to_string())
    } else {
        Ok(())
    }
}

/// Fetch the OSM data for one tile's padded bbox.
///
/// **This is the single fetch seam.** Everything that decides where OSM bytes come from lives
/// here and nowhere else, so a future `.osm.pbf` reader plugs in by adding one arm to this match
/// — no other part of the tile pipeline needs to know. `local_osm_file` already exercises that
/// seam with the existing `.osm`/`.xml` reader.
fn fetch_osm(bbox: &LLBBox, cfg: &TileJobConfig) -> Result<OsmData, String> {
    if cfg.args.skip_objects() {
        // Terrain-only: the pipeline never looks at objects, so don't pay for a fetch.
        return Ok(OsmData::empty());
    }
    match cfg.local_osm_file.as_deref() {
        Some(path) => retrieve_data::fetch_data_from_file(path)
            .map(|(data, _bounds)| data)
            .map_err(|e| format!("failed to load OSM file '{path}': {e}")),
        None => retrieve_data::fetch_data_from_overpass(
            *bbox,
            false,
            cfg.args.downloader.as_str(),
            None,
        )
        .map_err(|e| format!("failed to fetch OSM data: {e}")),
    }
}

/// Lat/lon envelope of a block rect, under `proj`.
///
/// All four corners are inverse-projected, not just two: transverse Mercator is not axis-aligned
/// in lat/lon, so the rect's north edge bows and its west edge leans. Taking the envelope of two
/// opposite corners would under-cover the bbox and leave slivers of the tile with no data.
fn rect_to_llbbox(rect: &BlockRect, proj: &TransverseMercatorProjection) -> Result<LLBBox, String> {
    let corners = [
        (rect.min_x, rect.min_z),
        (rect.max_x, rect.min_z),
        (rect.min_x, rect.max_z),
        (rect.max_x, rect.max_z),
    ];

    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;

    for (x, z) in corners {
        let (lat, lon) = proj.inverse(x as f64, z as f64);
        if !lat.is_finite() || !lon.is_finite() {
            return Err(format!(
                "tile rect corner ({x}, {z}) does not inverse-project to a valid position"
            ));
        }
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
    }

    LLBBox::new(min_lat, min_lon, max_lat, max_lon)
        .map_err(|e| format!("tile bbox is not a valid geographic rectangle: {e}"))
}

/// Build the terrain layer for one padded tile bbox.
///
/// The two things that make this different from the whole-world path in
/// `ground::generate_ground_data`: the world extent is passed explicitly (the bbox has already
/// been projected, so deriving the extent from haversine distance would misalign the grid), and
/// the vertical mapping is absolute (so the same elevation maps to the same Y in every tile
/// instead of being normalised to each tile's own relief).
fn build_ground(bbox: &LLBBox, xzbbox: &XZBBox, cfg: &TileJobConfig) -> Ground {
    let world_extent = Some((
        (xzbbox.max_x() - xzbbox.min_x() + 1).max(1) as usize,
        (xzbbox.max_z() - xzbbox.min_z() + 1).max(1) as usize,
    ));

    let ground = if cfg.args.terrain() {
        Ground::new_enabled(
            bbox,
            cfg.args.scale,
            cfg.vertical.sea_level_y,
            cfg.world_min_y + 2,
            cfg.world_min_y < crate::world_editor::DEFAULT_MIN_Y,
            cfg.world_max_y(),
            cfg.args.aws_only_elevation,
            false,
            cfg.args.canopy_height,
            world_extent,
            Some(cfg.vertical),
        )
    } else {
        // The flat-ground configuration skips every raster fetch. `new_flat_with_land_cover`
        // still downloads land cover (and canopy), which would defeat the point and make an
        // offline tile impossible, so it is only used when the client asked for those layers.
        Ground::new_flat(cfg.vertical.sea_level_y)
    };

    // Process globals the generator reads back out: the bedrock plane and the filler base. Safe
    // to retune per job only because generation is serialized onto one worker thread.
    crate::world_editor::set_base_chunk_y(ground.base_level());
    crate::world_editor::set_terrain_floor_y(ground.base_level());

    ground
}

/// Generate one tile: fetch, parse, terrain, generate, encode.
///
/// `progress` is called with the wire stage names (`fetching_osm`, `fetching_elevation`,
/// `generating`, `encoding`) as the job moves through them. `cancel` is checked between stages —
/// the pipeline itself is not interruptible, so a cancel takes effect at the next boundary and
/// surfaces as `Err("cancelled")`.
pub fn generate_tile(
    key: TileKey,
    anchor: &Anchor,
    session: &TileJobConfig,
    cancel: &AtomicBool,
    progress: &dyn Fn(&str),
) -> Result<GeneratedTile, String> {
    check_cancel(cancel)?;

    // (a) The tile we keep, and the padded rect we generate.
    let strict = key.rect(session.tile_size);
    let padded = strict.expand(session.margin);

    // (b) Its lat/lon envelope, through the anchor's pinned projection.
    let proj = anchor.projection();
    let bbox = rect_to_llbbox(&padded, &proj)?;

    // The dimension the client declared. A process global, retuned per job like the rest, and
    // safe to write only because this runs on the single generation worker while it holds the
    // process-wide `GenerationSlot` — no handshake, no second tile job and no disk generation
    // can be reading these bounds at the same time.
    crate::world_editor::set_world_bounds(session.world_min_y, session.world_max_y());

    check_cancel(cancel)?;

    // (c) OSM, through the single fetch seam.
    progress("fetching_osm");
    let osm = fetch_osm(&bbox, session)?;
    check_cancel(cancel)?;

    // (d) Parse with the PINNED projection, so element coordinates come out absolute and the
    // same real place lands on the same block in every tile that contains it.
    let (mut elements, xzbbox, outline_suppression, part_groups) =
        osm_parser::parse_osm_data_pinned(osm, bbox, session.args.scale, false, &proj);
    elements.sort_by_key(osm_parser::get_priority);
    check_cancel(cancel)?;

    // (e) Terrain, mapped absolutely against the same frame.
    progress("fetching_elevation");
    let ground = build_ground(&bbox, &xzbbox, session);
    let clipped = ground.terrain_clipped();
    check_cancel(cancel)?;

    // (f) The ordinary pipeline, in memory.
    progress("generating");
    let ground_origin = (xzbbox.min_x(), xzbbox.min_z());
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let options = GenerationOptions {
        path: std::path::PathBuf::new(),
        format: WorldFormat::JavaAnvil,
        level_name: None,
        spawn_point: None,
        luanti_game: None,
        ground_level: session.vertical.sea_level_y,
        sink: GenerationSink::Memory,
    };
    // `Ground` is needed again for the biome pass, and the pipeline takes it by value.
    let ground_for_biomes = ground.clone();
    let world = data_processing::generate_world_in_memory(
        elements,
        xzbbox,
        bbox,
        ground,
        &session.args,
        options,
        outline_suppression,
        part_groups,
    )?;
    check_cancel(cancel)?;

    // (g) Keep the strict tile, throw the margin away.
    progress("encoding");
    let chunks = encode_world(
        &world,
        &strict,
        &ground_for_biomes,
        center_lat,
        ground_origin,
        clipped,
    );

    Ok(GeneratedTile { chunks, clipped })
}

// ---------------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------------

/// Walk the generated world and encode every chunk that lies wholly inside `strict`.
fn encode_world(
    world: &WorldToModify,
    strict: &BlockRect,
    ground: &Ground,
    center_lat: f64,
    ground_origin: (i32, i32),
    clipped: bool,
) -> FnvHashMap<(i32, i32), ChunkPayload> {
    let mut out: FnvHashMap<(i32, i32), ChunkPayload> = FnvHashMap::default();

    for (&(region_x, region_z), region) in &world.regions {
        for (&(local_x, local_z), chunk) in &region.chunks {
            // Region key is `chunk >> 5`, chunk key inside it is `chunk & 31`; this rebuilds the
            // absolute chunk coordinate, negatives included.
            let chunk_x = region_x * 32 + local_x;
            let chunk_z = region_z * 32 + local_z;
            if !strict.contains_chunk(chunk_x, chunk_z) {
                continue;
            }
            let payload = encode_chunk(
                chunk,
                chunk_x,
                chunk_z,
                ground,
                center_lat,
                ground_origin,
                clipped,
            );
            out.insert((chunk_x, chunk_z), payload);
        }
    }

    out
}

/// Encode one chunk into its wire payload.
fn encode_chunk(
    chunk: &ChunkToModify,
    chunk_x: i32,
    chunk_z: i32,
    ground: &Ground,
    center_lat: f64,
    ground_origin: (i32, i32),
    clipped: bool,
) -> ChunkPayload {
    // Sections are stored sparsely; the wire format is a contiguous run, so fill the gaps with
    // `Empty` and then trim the empty ends off.
    let mut sections: Vec<SectionPayload> = Vec::new();
    let mut min_section_y = 0i32;

    if let (Some(&lo), Some(&hi)) = (chunk.sections.keys().min(), chunk.sections.keys().max()) {
        let mut encoded: Vec<SectionPayload> =
            Vec::with_capacity((hi as i32 - lo as i32 + 1) as usize);
        for y in lo..=hi {
            match chunk.sections.get(&y) {
                Some(section) => encoded.push(encode_section(section)),
                None => encoded.push(SectionPayload::Empty),
            }
        }

        // Trim leading/trailing air so a chunk with one building does not ship 24 empty sections.
        let first = encoded
            .iter()
            .position(|s| !matches!(s, SectionPayload::Empty));
        if let Some(first) = first {
            let last = encoded
                .iter()
                .rposition(|s| !matches!(s, SectionPayload::Empty))
                .unwrap_or(first);
            min_section_y = lo as i32 + first as i32;
            sections = encoded.drain(first..=last).collect();
        }
    }

    let biomes: [String; 16] = crate::biome::chunk_biome_palette(
        chunk_x,
        chunk_z,
        Some(ground),
        center_lat,
        ground_origin,
    )
    .map(|s| s.to_string());

    let block_entities: Vec<BlockEntityPayload> = chunk
        .other
        .get("block_entities")
        .and_then(|v| match v {
            Value::List(list) => Some(list),
            _ => None,
        })
        .map(|list| list.iter().filter_map(semantic_block_entity).collect())
        .unwrap_or_default();

    ChunkPayload {
        chunk_x,
        chunk_z,
        clipped,
        min_section_y,
        sections,
        biomes,
        block_entities,
    }
}

/// Encode one 16x16x16 section.
///
/// Blocks are read out of [`BlockStorage`] directly rather than going through
/// `SectionToModify::to_section`: that path formats names with `Block::name()`, which panics on an
/// id the palette never assigned. Here an unassigned id is treated as air and skipped, which is
/// the only safe thing to do when the alternative is taking the whole server down.
fn encode_section(section: &SectionToModify) -> SectionPayload {
    // Fast path: one block everywhere and no per-cell properties.
    if section.properties.is_empty() {
        if let BlockStorage::Uniform(block) = &section.storage {
            return match block_state(*block, None) {
                Some(name) => SectionPayload::Uniform(name),
                None => SectionPayload::Empty,
            };
        }
    }

    // Index 0 is always air: skipped cells and unassigned ids fall back to it.
    let mut palette: Vec<String> = vec!["minecraft:air".to_string()];
    let mut by_name: FnvHashMap<String, u16> = FnvHashMap::default();
    by_name.insert("minecraft:air".to_string(), 0);
    // Memo keyed by (block id, per-cell property identity), so the common case never formats a
    // blockstate string twice. 0 means "no per-cell properties".
    let mut memo: FnvHashMap<(u16, usize), u16> = FnvHashMap::default();

    let mut indices: Vec<u16> = Vec::with_capacity(SECTION_CELLS);
    let mut non_air = 0usize;

    for i in 0..SECTION_CELLS {
        let block = section.storage.get(i);
        let props = section.properties.get(&i);
        let props_key = props.map_or(0usize, |p| Arc::as_ptr(p) as usize);

        let index = match memo.get(&(block.id(), props_key)) {
            Some(&idx) => idx,
            None => {
                let idx = match block_state(block, props.map(|p| p.as_ref())) {
                    Some(name) => match by_name.get(&name) {
                        Some(&existing) => existing,
                        None => {
                            let new_idx = palette.len() as u16;
                            by_name.insert(name.clone(), new_idx);
                            palette.push(name);
                            new_idx
                        }
                    },
                    None => 0,
                };
                memo.insert((block.id(), props_key), idx);
                idx
            }
        };

        if index != 0 {
            non_air += 1;
        }
        indices.push(index);
    }

    if non_air == 0 {
        return SectionPayload::Empty;
    }

    SectionPayload::Paletted { palette, indices }
}

/// The blockstate string for one block, or `None` when it is air or an id with no name.
///
/// Per-cell properties (the ones the generator attached to this exact position) win over the
/// block's own default properties.
fn block_state(block: Block, cell_props: Option<&Value>) -> Option<String> {
    if block == AIR {
        return None;
    }
    let name = block.try_name()?;
    let qualified = format!("{}:{}", block.namespace(), name);
    match cell_props {
        Some(props) => Some(blockstate_string(&qualified, Some(props))),
        None => {
            let own = block.properties();
            Some(blockstate_string(&qualified, own.as_ref()))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Block entities
// ---------------------------------------------------------------------------------------------

/// Convert one raw block-entity NBT compound into its semantic wire form.
///
/// Arnis emits block entities as fastnbt `Value`s shaped for the Java Anvil writer. None of that
/// goes on the wire: NBT layouts differ between Minecraft versions, and keeping those differences
/// on the mod side is the whole point of the semantic form. Anything this converter does not
/// recognise is dropped silently — a block entity the mod cannot place is not worth a protocol
/// error, and the block itself is already in the section data.
pub fn semantic_block_entity(value: &Value) -> Option<BlockEntityPayload> {
    let Value::Compound(map) = value else {
        return None;
    };

    let id = match map.get("id") {
        Some(Value::String(s)) => s.as_str(),
        _ => return None,
    };
    let x = nbt_i32(map.get("x"))?;
    let y = nbt_i32(map.get("y"))?;
    let z = nbt_i32(map.get("z"))?;

    let bare = id.strip_prefix("minecraft:").unwrap_or(id);
    let (kind, body) = match bare {
        s if s == "sign" || s.ends_with("_sign") => ("sign", sign_body(map)),
        "chest" | "trapped_chest" => ("chest", chest_body(map)),
        "banner" => ("banner", banner_body(map)),
        "bed" => ("bed", serde_json::json!({})),
        "item_frame" | "glow_item_frame" => ("item_frame", item_frame_body(map)),
        _ => return None,
    };

    Some(BlockEntityPayload {
        x,
        y,
        z,
        kind: kind.to_string(),
        data: body,
    })
}

/// `{"lines": [.., .., .., ..], "color": "black", "glowing": false}`, plus `facing` when the NBT
/// carries one.
fn sign_body(map: &std::collections::HashMap<String, Value>) -> serde_json::Value {
    let front = match map.get("front_text") {
        Some(Value::Compound(c)) => Some(c),
        _ => None,
    };

    let mut lines = vec![String::new(), String::new(), String::new(), String::new()];
    let mut color = "black".to_string();
    let mut glowing = false;

    if let Some(front) = front {
        if let Some(Value::List(messages)) = front.get("messages") {
            for (i, message) in messages.iter().take(4).enumerate() {
                if let Value::String(raw) = message {
                    lines[i] = plain_text(raw);
                }
            }
        }
        if let Some(Value::String(c)) = front.get("color") {
            color = c.clone();
        }
        if let Some(Value::Byte(b)) = front.get("has_glowing_text") {
            glowing = *b != 0;
        }
    }

    let mut body = serde_json::json!({
        "lines": lines,
        "color": color,
        "glowing": glowing,
    });
    if let Some(facing) = nbt_i32(map.get("Facing")).or_else(|| nbt_i32(map.get("facing"))) {
        body["facing"] = serde_json::json!(facing);
    }
    body
}

/// `{"items": [{"id": "minecraft:..", "count": n}, ..]}` — item ids and counts only, never the
/// full item NBT.
fn chest_body(map: &std::collections::HashMap<String, Value>) -> serde_json::Value {
    let mut items: Vec<serde_json::Value> = Vec::new();
    if let Some(Value::List(list)) = map.get("Items").or_else(|| map.get("items")) {
        for entry in list {
            let Value::Compound(item) = entry else {
                continue;
            };
            let Some(Value::String(id)) = item.get("id") else {
                continue;
            };
            let count = nbt_i32(item.get("count"))
                .or_else(|| nbt_i32(item.get("Count")))
                .unwrap_or(1);
            items.push(serde_json::json!({ "id": id, "count": count }));
        }
    }
    serde_json::json!({ "items": items })
}

/// `{"patterns": [{"color": "..", "pattern": "minecraft:.."}, ..]}`.
fn banner_body(map: &std::collections::HashMap<String, Value>) -> serde_json::Value {
    let mut patterns: Vec<serde_json::Value> = Vec::new();
    if let Some(Value::List(list)) = map.get("patterns") {
        for entry in list {
            let Value::Compound(p) = entry else { continue };
            let color = match p.get("color") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            let pattern = match p.get("pattern") {
                Some(Value::String(s)) => s.clone(),
                _ => continue,
            };
            patterns.push(serde_json::json!({ "color": color, "pattern": pattern }));
        }
    }
    serde_json::json!({ "patterns": patterns })
}

/// `{"facing": n}` when the frame declares one, otherwise `{}`.
fn item_frame_body(map: &std::collections::HashMap<String, Value>) -> serde_json::Value {
    match nbt_i32(map.get("Facing")).or_else(|| nbt_i32(map.get("facing"))) {
        Some(facing) => serde_json::json!({ "facing": facing }),
        None => serde_json::json!({}),
    }
}

/// Read any of NBT's integer widths as `i32`.
fn nbt_i32(value: Option<&Value>) -> Option<i32> {
    match value? {
        Value::Byte(b) => Some(*b as i32),
        Value::Short(s) => Some(*s as i32),
        Value::Int(i) => Some(*i),
        Value::Long(l) => Some(*l as i32),
        _ => None,
    }
}

/// Flatten a Minecraft JSON text component down to its literal text.
///
/// Sign lines are stored as JSON component strings (`"\"Main Street\""`), which is a rendering
/// detail of one Minecraft version. The mod gets the words.
fn plain_text(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Object(o)) => o
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => raw.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------------------------

/// A bounded, least-recently-used cache of generated tiles.
///
/// Deliberately hand-rolled from a map plus a `VecDeque` of keys: the capacity is a handful of
/// entries and every operation happens on one thread behind the session lock, so an LRU crate
/// would be a dependency bought for nothing.
pub struct TileCache {
    capacity: usize,
    /// Hash of the config the resident tiles were generated under.
    config_hash: u64,
    tiles: FnvHashMap<TileKey, GeneratedTile>,
    /// Recency, least-recently-used at the front.
    order: VecDeque<TileKey>,
    hits: u64,
    misses: u64,
}

impl TileCache {
    /// An empty cache holding at most `capacity` tiles (at least one).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            config_hash: 0,
            tiles: FnvHashMap::default(),
            order: VecDeque::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// Point the cache at a generation config, dropping every resident tile if it changed.
    ///
    /// Wholesale, not per entry: a config change invalidates everything, and a cache that served
    /// one stale tile would show up as a seam in the world rather than as an error.
    pub fn set_config_hash(&mut self, hash: u64) {
        if self.config_hash != hash {
            self.config_hash = hash;
            self.tiles.clear();
            self.order.clear();
        }
    }

    /// The config hash the resident tiles were generated under.
    #[allow(dead_code)]
    pub fn config_hash(&self) -> u64 {
        self.config_hash
    }

    /// Look up a tile, refreshing its recency on a hit.
    pub fn get(&mut self, key: TileKey) -> Option<&GeneratedTile> {
        if !self.tiles.contains_key(&key) {
            self.misses += 1;
            return None;
        }
        self.touch(&key);
        self.hits += 1;
        self.tiles.get(&key)
    }

    /// Whether a tile is resident, without touching recency or the counters.
    pub fn contains(&self, key: &TileKey) -> bool {
        self.tiles.contains_key(key)
    }

    /// Store a tile, evicting the least recently used one if the cache is full.
    pub fn insert(&mut self, key: TileKey, tile: GeneratedTile) {
        if self.tiles.insert(key, tile).is_some() {
            // Replacing an existing entry: it keeps its slot, but becomes the most recent.
            self.touch(&key);
            return;
        }
        self.order.push_back(key);
        while self.tiles.len() > self.capacity {
            match self.order.pop_front() {
                Some(evicted) => {
                    self.tiles.remove(&evicted);
                }
                None => break,
            }
        }
    }

    /// Fraction of lookups that hit, or `0.0` before the first lookup.
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }

    /// Lookups that hit.
    #[allow(dead_code)]
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Lookups that missed.
    #[allow(dead_code)]
    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// Resident tiles.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Whether no tile is resident.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Move `key` to the most-recently-used end.
    fn touch(&mut self, key: &TileKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(*key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::collections::HashMap;

    const TS: i32 = 512;

    /// `overflow-checks` is on in release too, so a plain `c * 16` here is a panic on whichever
    /// thread a client-supplied chunk coordinate reaches, not a wrong number.
    #[test]
    fn extreme_chunk_coordinates_saturate_instead_of_overflowing() {
        assert_eq!(chunk_min_block(0), 0);
        assert_eq!(chunk_min_block(-1), -16);
        assert_eq!(chunk_min_block(i32::MAX), i32::MAX);
        assert_eq!(chunk_min_block(i32::MIN), i32::MIN);
        // 2^27 is the first chunk coordinate whose block origin does not fit in an i32.
        assert_eq!(chunk_min_block(134_217_728), i32::MAX);

        assert_eq!(tile_of_chunk(0, TS), 0);
        assert_eq!(tile_of_chunk(-1, TS), -1);
        assert_eq!(tile_of_chunk(i32::MAX, TS), tile_of_block(i32::MAX, TS));
        assert_eq!(tile_of_chunk(i32::MIN, TS), tile_of_block(i32::MIN, TS));
    }

    fn key(tx: i32, tz: i32) -> TileKey {
        TileKey {
            anchor_id: 1,
            tx,
            tz,
        }
    }

    fn empty_tile() -> GeneratedTile {
        GeneratedTile {
            chunks: FnvHashMap::default(),
            clipped: false,
        }
    }

    fn test_config() -> TileJobConfig {
        TileJobConfig {
            args: Arc::new(Args::parse_from(["arnis"])),
            vertical: AbsoluteVerticalMapping {
                sea_level_y: 62,
                blocks_per_meter: 1.0,
            },
            world_min_y: -64,
            world_height: 384,
            local_osm_file: None,
            tile_size: TS,
            margin: 128,
            anchors: vec![AnchorDigest {
                id: 1,
                origin_lat: 48.8566,
                origin_lon: 2.3522,
                scale: 1.0,
                mc_x: 0.0,
                mc_z: 0.0,
            }],
        }
    }

    // --- coordinates -------------------------------------------------------------------------

    /// The highest-value test in the file: negative coordinates are the NORMAL case in stream
    /// mode, and truncating division would silently shift every tile west or north of the anchor.
    #[test]
    fn tile_of_block_handles_negatives_and_boundaries() {
        // Positive side.
        assert_eq!(tile_of_block(0, TS), 0);
        assert_eq!(tile_of_block(1, TS), 0);
        assert_eq!(tile_of_block(511, TS), 0);
        // Exactly on a boundary: the boundary block belongs to the HIGHER tile.
        assert_eq!(tile_of_block(512, TS), 1);
        assert_eq!(tile_of_block(1023, TS), 1);
        assert_eq!(tile_of_block(1024, TS), 2);

        // Negative side. -1 must be tile -1, not tile 0.
        assert_eq!(tile_of_block(-1, TS), -1);
        assert_eq!(tile_of_block(-511, TS), -1);
        assert_eq!(tile_of_block(-512, TS), -1);
        assert_eq!(tile_of_block(-513, TS), -2);
        assert_eq!(tile_of_block(-1024, TS), -2);
        assert_eq!(tile_of_block(-1025, TS), -3);
    }

    #[test]
    fn tile_rect_round_trips_every_block_it_covers() {
        for tx in -3..=3 {
            for tz in -3..=3 {
                let rect = key(tx, tz).rect(TS);
                assert_eq!(rect.width(), TS);
                assert_eq!(rect.depth(), TS);
                // Every corner and the centre map back to this tile.
                for (x, z) in [
                    (rect.min_x, rect.min_z),
                    (rect.max_x - 1, rect.min_z),
                    (rect.min_x, rect.max_z - 1),
                    (rect.max_x - 1, rect.max_z - 1),
                    (rect.min_x + TS / 2, rect.min_z + TS / 2),
                ] {
                    assert_eq!(tile_of_block(x, TS), tx, "block x {x}");
                    assert_eq!(tile_of_block(z, TS), tz, "block z {z}");
                    assert_eq!(TileKey::from_block(1, x, z, TS), key(tx, tz));
                }
                // One block past the far edge belongs to the next tile, with no gap.
                assert_eq!(tile_of_block(rect.max_x, TS), tx + 1);
                assert_eq!(tile_of_block(rect.min_x - 1, TS), tx - 1);
            }
        }
    }

    #[test]
    fn chunk_and_tile_conversions_agree_across_the_origin() {
        for cx in -40..40 {
            let block = chunk_min_block(cx);
            assert_eq!(chunk_of_block(block), cx);
            assert_eq!(chunk_of_block(block + 15), cx);
            assert_eq!(tile_of_chunk(cx, TS), tile_of_block(block, TS));
            assert_eq!(
                TileKey::from_chunk(7, cx, cx, TS),
                TileKey::from_block(7, block, block, TS)
            );
        }
        // Chunk -1 covers blocks -16..-1 and lives in tile -1.
        assert_eq!(chunk_of_block(-1), -1);
        assert_eq!(chunk_of_block(-16), -1);
        assert_eq!(chunk_of_block(-17), -2);
        assert_eq!(tile_of_chunk(-1, TS), -1);
        // The last chunk of tile -1 is chunk -1; the first chunk of tile -2 is chunk -32.
        assert_eq!(tile_of_chunk(-32, TS), -1);
        assert_eq!(tile_of_chunk(-33, TS), -2);
    }

    #[test]
    fn tiles_partition_the_axis_with_no_gap_or_overlap() {
        let mut prev = key(-4, 0).rect(TS);
        for tx in -3..=4 {
            let rect = key(tx, 0).rect(TS);
            assert_eq!(prev.max_x, rect.min_x, "gap or overlap before tile {tx}");
            prev = rect;
        }
    }

    #[test]
    fn padded_rect_adds_the_margin_on_every_side() {
        const MARGIN: i32 = 128;
        for (tx, tz) in [(0, 0), (-1, -1), (3, -7)] {
            let strict = key(tx, tz).rect(TS);
            let padded = strict.expand(MARGIN);
            assert_eq!(padded.min_x, strict.min_x - MARGIN);
            assert_eq!(padded.min_z, strict.min_z - MARGIN);
            assert_eq!(padded.max_x, strict.max_x + MARGIN);
            assert_eq!(padded.max_z, strict.max_z + MARGIN);
            assert_eq!(padded.width(), TS + 2 * MARGIN);
            assert_eq!(padded.depth(), TS + 2 * MARGIN);
        }
    }

    #[test]
    fn contains_chunk_rejects_chunks_straddling_the_edge() {
        let rect = key(0, 0).rect(TS);
        assert!(rect.contains_chunk(0, 0));
        assert!(rect.contains_chunk(31, 31));
        assert!(!rect.contains_chunk(32, 0));
        assert!(!rect.contains_chunk(-1, 0));

        let negative = key(-1, -1).rect(TS);
        assert!(negative.contains_chunk(-1, -1));
        assert!(negative.contains_chunk(-32, -32));
        assert!(!negative.contains_chunk(-33, -1));
    }

    #[test]
    fn to_xzbbox_is_inclusive_and_matches_the_rect() {
        let rect = key(-1, 2).rect(TS);
        let bbox = rect.to_xzbbox().unwrap();
        assert_eq!(bbox.min_x(), rect.min_x);
        assert_eq!(bbox.min_z(), rect.min_z);
        assert_eq!(bbox.max_x(), rect.max_x - 1);
        assert_eq!(bbox.max_z(), rect.max_z - 1);
    }

    #[test]
    fn tile_size_is_snapped_to_a_chunk_multiple() {
        assert_eq!(DEFAULT_TILE_SIZE % CHUNK_BLOCKS, 0);
        assert_eq!(DEFAULT_MARGIN % CHUNK_BLOCKS, 0);
    }

    // --- cache -------------------------------------------------------------------------------

    #[test]
    fn cache_evicts_the_least_recently_used_tile() {
        let mut cache = TileCache::new(2);
        cache.insert(key(0, 0), empty_tile());
        cache.insert(key(1, 0), empty_tile());
        cache.insert(key(2, 0), empty_tile());

        assert_eq!(cache.len(), 2);
        assert!(!cache.contains(&key(0, 0)), "oldest tile should be evicted");
        assert!(cache.contains(&key(1, 0)));
        assert!(cache.contains(&key(2, 0)));
    }

    #[test]
    fn get_refreshes_recency() {
        let mut cache = TileCache::new(2);
        cache.insert(key(0, 0), empty_tile());
        cache.insert(key(1, 0), empty_tile());

        // Touch the older tile, so the newer one becomes the eviction candidate.
        assert!(cache.get(key(0, 0)).is_some());
        cache.insert(key(2, 0), empty_tile());

        assert!(cache.contains(&key(0, 0)), "refreshed tile should survive");
        assert!(!cache.contains(&key(1, 0)));
        assert!(cache.contains(&key(2, 0)));
    }

    #[test]
    fn hit_rate_tracks_hits_and_misses() {
        let mut cache = TileCache::new(4);
        assert_eq!(cache.hit_rate(), 0.0);

        cache.insert(key(0, 0), empty_tile());
        assert!(cache.get(key(0, 0)).is_some());
        assert!(cache.get(key(9, 9)).is_none());

        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert!((cache.hit_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn config_change_clears_the_cache() {
        let mut cache = TileCache::new(4);
        cache.set_config_hash(1);
        cache.insert(key(0, 0), empty_tile());
        assert_eq!(cache.len(), 1);

        // Same hash: nothing happens.
        cache.set_config_hash(1);
        assert_eq!(cache.len(), 1);

        // Different hash: everything goes.
        cache.set_config_hash(2);
        assert!(cache.is_empty());
        assert_eq!(cache.config_hash(), 2);
    }

    #[test]
    fn reinserting_a_key_does_not_grow_the_cache() {
        let mut cache = TileCache::new(2);
        cache.insert(key(0, 0), empty_tile());
        cache.insert(key(0, 0), empty_tile());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.order.len(), 1);
    }

    // --- config hash -------------------------------------------------------------------------

    #[test]
    fn config_hash_is_stable_for_equal_input() {
        let a = test_config();
        let b = test_config();
        assert_eq!(config_hash(&a), config_hash(&b));
        assert_eq!(config_hash(&a), config_hash(&a));
    }

    #[test]
    fn config_hash_changes_with_every_contributing_field() {
        let base = config_hash(&test_config());

        let mut cfg = test_config();
        cfg.tile_size = 256;
        assert_ne!(config_hash(&cfg), base, "tile size");

        let mut cfg = test_config();
        cfg.margin = 64;
        assert_ne!(config_hash(&cfg), base, "margin");

        let mut cfg = test_config();
        cfg.vertical.sea_level_y = 63;
        assert_ne!(config_hash(&cfg), base, "sea level");

        let mut cfg = test_config();
        cfg.vertical.blocks_per_meter = 1.5;
        assert_ne!(config_hash(&cfg), base, "vertical scale");

        let mut cfg = test_config();
        cfg.world_min_y = -2032;
        assert_ne!(config_hash(&cfg), base, "world floor");

        let mut cfg = test_config();
        cfg.world_height = 4064;
        assert_ne!(config_hash(&cfg), base, "world height");

        let mut cfg = test_config();
        cfg.local_osm_file = Some("/tmp/area.osm".to_string());
        assert_ne!(config_hash(&cfg), base, "local osm file");

        let mut cfg = test_config();
        cfg.args = Arc::new(Args::parse_from(["arnis", "--scale", "2.0"]));
        assert_ne!(config_hash(&cfg), base, "scale");

        let mut cfg = test_config();
        cfg.args = Arc::new(Args::parse_from(["arnis", "--fillground"]));
        assert_ne!(config_hash(&cfg), base, "fillground");

        // Anchor identity: moving the anchor moves the whole world under the tiles.
        let mut cfg = test_config();
        cfg.anchors[0].mc_x = 1024.0;
        assert_ne!(config_hash(&cfg), base, "anchor position");

        let mut cfg = test_config();
        cfg.anchors[0].origin_lat = 48.9;
        assert_ne!(config_hash(&cfg), base, "anchor latitude");

        let mut cfg = test_config();
        cfg.anchors.push(AnchorDigest {
            id: 2,
            origin_lat: 51.5,
            origin_lon: -0.12,
            scale: 1.0,
            mc_x: 4096.0,
            mc_z: 0.0,
        });
        assert_ne!(config_hash(&cfg), base, "anchor added");
    }

    // --- block entities ----------------------------------------------------------------------

    fn compound(entries: Vec<(&str, Value)>) -> Value {
        let mut map: HashMap<String, Value> = HashMap::new();
        for (k, v) in entries {
            map.insert(k.to_string(), v);
        }
        Value::Compound(map)
    }

    #[test]
    fn sign_nbt_becomes_the_semantic_form() {
        let front = compound(vec![
            (
                "messages",
                Value::List(vec![
                    Value::String("\"Main Street\"".to_string()),
                    Value::String("\"\"".to_string()),
                    Value::String("\"\"".to_string()),
                    Value::String("\"\"".to_string()),
                ]),
            ),
            ("color", Value::String("black".to_string())),
            ("has_glowing_text", Value::Byte(0)),
        ]);
        let be = compound(vec![
            ("id", Value::String("minecraft:sign".to_string())),
            ("x", Value::Int(12)),
            ("y", Value::Int(70)),
            ("z", Value::Int(-34)),
            ("is_waxed", Value::Byte(1)),
            ("front_text", front),
        ]);

        let payload = semantic_block_entity(&be).expect("sign should convert");
        assert_eq!(payload.kind, "sign");
        assert_eq!((payload.x, payload.y, payload.z), (12, 70, -34));

        let json = &payload.data;
        assert_eq!(json["lines"][0], "Main Street");
        assert_eq!(json["lines"][3], "");
        assert_eq!(json["color"], "black");
        assert_eq!(json["glowing"], false);
        // The NBT layout must not leak through.
        let raw = payload.data.to_string();
        assert!(!raw.contains("front_text"));
        assert!(!raw.contains("is_waxed"));
    }

    #[test]
    fn unrecognised_block_entity_is_skipped_without_erroring() {
        let be = compound(vec![
            ("id", Value::String("minecraft:beacon".to_string())),
            ("x", Value::Int(0)),
            ("y", Value::Int(0)),
            ("z", Value::Int(0)),
        ]);
        assert!(semantic_block_entity(&be).is_none());

        // Malformed entries are equally harmless.
        assert!(semantic_block_entity(&Value::Int(3)).is_none());
        assert!(semantic_block_entity(&compound(vec![("x", Value::Int(0))])).is_none());
        assert!(semantic_block_entity(&compound(vec![(
            "id",
            Value::String("minecraft:sign".to_string())
        )]))
        .is_none());
    }

    #[test]
    fn bed_and_banner_convert_to_their_kinds() {
        let bed = compound(vec![
            ("id", Value::String("minecraft:bed".to_string())),
            ("x", Value::Int(1)),
            ("y", Value::Int(2)),
            ("z", Value::Int(3)),
        ]);
        assert_eq!(semantic_block_entity(&bed).unwrap().kind, "bed");

        let banner = compound(vec![
            ("id", Value::String("minecraft:banner".to_string())),
            ("x", Value::Int(1)),
            ("y", Value::Int(2)),
            ("z", Value::Int(3)),
            (
                "patterns",
                Value::List(vec![compound(vec![
                    ("color", Value::String("red".to_string())),
                    (
                        "pattern",
                        Value::String("minecraft:triangle_top".to_string()),
                    ),
                ])]),
            ),
        ]);
        let payload = semantic_block_entity(&banner).unwrap();
        assert_eq!(payload.kind, "banner");
        let json = &payload.data;
        assert_eq!(json["patterns"][0]["color"], "red");
        assert_eq!(json["patterns"][0]["pattern"], "minecraft:triangle_top");
    }

    #[test]
    fn plain_text_unwraps_json_components() {
        assert_eq!(plain_text("\"hello\""), "hello");
        assert_eq!(plain_text("{\"text\":\"hello\"}"), "hello");
        // Not valid JSON at all: pass it through rather than losing the text.
        assert_eq!(plain_text("hello"), "hello");
    }
}
