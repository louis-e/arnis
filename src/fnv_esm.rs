use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::deterministic_rng::coord_rng;
use crate::ground::Ground;
use crate::land_cover;
use colored::Colorize;
use rand::Rng;
use std::collections::BTreeMap;
use std::io;
use std::io::Write;
use std::fs;
use std::path::Path;

/// Returns a non-zero priority for driveable highway types that should have
/// the asphalt texture painted on FNV terrain, or 0 for non-road types.
pub(crate) fn fnv_road_type(highway: &str) -> u8 {
    match highway {
        "motorway" | "motorway_link" => 4,
        "trunk" | "trunk_link" | "primary" | "primary_link" => 3,
        "secondary" | "secondary_link" | "tertiary" | "tertiary_link" => 2,
        "residential" | "living_street" | "unclassified" | "service" | "road" => 1,
        _ => 0,
    }
}

const BLOCKS_PER_CELL: i32 = 32;
const VERTS: usize = 33;
const HEIGHT_MARGIN: i32 = 16;
const FNV_FORM_VERSION: u16 = 15;

// ---------------------------------------------------------------------------
// Terrain smoothing
//
// Raw DEM data often has sharp stair-step artefacts that look fine as
// Minecraft blocks but are jarring as a continuous FNV landscape.  We apply
// an iterative gradient-limited Laplacian smooth to the sampled height grid
// before encoding VHGT deltas.
//
// To guarantee that adjacent cells produce identical heights at their shared
// border vertices, the smooth is applied to a padded grid that extends
// SMOOTH_PAD vertices beyond each edge.  Both cells call ground.level() at
// the same coordinates for this border region, so all inputs are identical
// and the deterministic algorithm yields the same smoothed border heights.
// ---------------------------------------------------------------------------

/// Extra vertex rows/columns sampled outside each cell edge.
/// Must be ≥ SMOOTH_ITERS so that every inner vertex's smoothed value depends
/// only on raw ground.level() samples, guaranteeing inter-cell consistency.
const SMOOTH_PAD: usize = 7;

/// Number of smoothing passes.
const SMOOTH_ITERS: usize = 6;

/// VHGT gradient (units per vertex) below which no smoothing is applied.
/// ~10 units ≈ atan(80/128) ≈ 32° slope — gentle hills are left unchanged.
const SMOOTH_THRESHOLD: i32 = 10;

/// VHGT gradient above which the maximum blend is applied.
/// ~32 units ≈ atan(256/128) ≈ 63° — near-vertical cliffs get full treatment.
const SMOOTH_FULL: i32 = 32;

/// Maximum fraction of the difference toward the 4-neighbour average applied
/// per iteration.  0.65 is aggressive enough to tame cliffs in a few passes
/// without over-flattening moderate slopes.
const SMOOTH_MAX_BLEND: f32 = 0.65;
/// Each FNV exterior cell spans 4096 game units in X and Y.
const CELL_GAME_UNITS: f32 = 4096.0;
/// Multiplier applied to terrain height variation for more dramatic FNV terrain.
/// Minecraft Y blocks (1m each) map to VHGT units (8 game units ≈ 11 cm each),
/// so raw values need scaling to produce visible elevation changes in-game.
const HEIGHT_SCALE: i32 = 5;

// ---------------------------------------------------------------------------
// Template strategy
//
// Building the TES4 header from scratch produces FormIDs that FNVEdit
// displays with a "00" prefix, treating the worldspace as a patch to a
// nonexistent base-game record.  Using testesm.esm's verified-working TES4
// bytes (embedded at compile time) avoids this entirely: we inherit its exact
// master-list encoding and FormID namespace, then replace only the WRLD/CELL/
// LAND content.
// ---------------------------------------------------------------------------

/// Raw bytes of testesm.esm, embedded at compile time.
const TESTESM_BYTES: &[u8] = include_bytes!("../testesm.esm");

/// Length of the TES4 record in testesm (24-byte record header + 66-byte data).
const TESTESM_TES4_LEN: usize = 90;

// Byte offsets *within the TES4 record bytes* for the two fields we patch:
//   record header (24): tag(4)+size(4)+flags(4)+fid(4)+vcs(4)+ver(2)+vc2(2)
//   HEDR subrecord (18): tag(4)+size_u16(2)+version_f32(4)+numRecords(4)+nextOID(4)
const TES4_NUM_RECORDS_OFFSET: usize = 24 + 4 + 2 + 4; // = 34
const TES4_NEXT_OID_OFFSET: usize = TES4_NUM_RECORDS_OFFSET + 4; // = 38

/// WRLD FormID as it exists in testesm.esm — displays correctly in FNVEdit.
const TESTESM_WRLD_FID: u32 = 0x01000ADD;

/// Lower-24-bit base for new FormIDs allocated by this generator.
/// Equals testesm's HEDR nextObjectID (0x0B1A), so we start just after
/// testesm's existing FormID space.
const TESTESM_NEXT_OID_BASE: u32 = 0x0000_0B1A;

/// High byte shared by all FormIDs in testesm's namespace.
const PLUGIN_IDX: u32 = 0x01_00_00_00;

// ---------------------------------------------------------------------------
// Terrain texture FormIDs
//
// These were painted by the user into cell (0,0) of arnis_worldspace.esm.
// All FormIDs reference textures in FalloutNV.esm (plugin index 0x00).
// Swap the assignments below if the textures appear in the wrong order.
// ---------------------------------------------------------------------------

/// Grass / natural ground surface texture.
const TEXTURE_GRASS: u32 = 0x000F9912;
/// Asphalt / road / built-up area surface texture.
const TEXTURE_ASPHALT: u32 = 0x000009CA;
/// Snow / ice surface texture.
const TEXTURE_SNOW: u32 = 0x00143ABF;
/// Sand
const TEXTURE_SAND: u32 = 0x00103FF4;
/// Dirt/Default
const TEXTURE_DIRT: u32 = 0x00000A8A;

// ---------------------------------------------------------------------------
// Static object FormIDs (all reference FalloutNV.esm, plugin index 0x00)
// ---------------------------------------------------------------------------

// Forest trees — placed in LC_TREE_COVER cells (random Cedar or Aspen per point).
const CEDAR_FID: u32 = 0x001479DF;
const ASPEN_FID: u32 = 0x001479DB;

// Desert trees — placed in LC_BARE cells (random Joshua Tree or Palm per point).
const JOSHUA_TREE_FID: u32 = 0x0008D47C;
const PALM_TREE_FID: u32 = 0x00111DA0;

/// Grid spacing (in arnis blocks) between candidate tree placement points.
const TREE_GRID_SPACING: i32 = 8;

// Rock variants — sizes are approximate model extents.
const ROCK_SMALL_CLUSTER_FID: u32 = 0x00119885; // ~2 m diameter cluster
const ROCK_TALL_FID: u32 = 0x00119882;           // ~2 m wide × 4 m tall
const ROCK_MEDIUM_FID: u32 = 0x00119887;         // ~4 m × 8 m
const ROCK_LARGE_FID: u32 = 0x00119888;          // ~8 m × 16 m

/// Grid spacing for rock placement.  Coarser than trees to avoid wall-of-rocks effect.
const ROCK_GRID_SPACING: i32 = 10;

// House models — placed in LC_BUILT_UP cells near roads.
// Each model is 744 game units per side (≈5.8 arnis blocks).
// Awning adds 241 units (≈1.9 blocks) of extra depth on one face.
const HOUSE1_FID: u32 = 0x000C_B15C; // awning on front; default front faces -X (west)
const HOUSE2_FID: u32 = 0x000C_B15E; // awning on front; default front faces -Y (south)
const HOUSE3_FID: u32 = 0x000C_B15F; // awning on back;  default front faces -Y (south)

/// Spacing along road polylines (arnis blocks) between house placement intervals.
/// Houses are attempted on both sides of every road at this interval.
const HOUSE_ROAD_SPACING: i32 = 10;

/// Distance beyond the road edge (arnis blocks) to the house centre.
/// Actual offset from road centreline = road_half_width + HOUSE_SETBACK.
const HOUSE_SETBACK: i32 = 5;

// Commercial buildings — placed alongside higher-priority roads (priority ≥ 2).
const COMM_2STORY_FID: u32 = 0x000F_1B07; // 1093×790 units,  entry on −Y axis
const COMM_1STORY_FID: u32 = 0x000E_EEAF; //  916×2258 units, entry on +Y axis
const COMM_4STORY_FID: u32 = 0x0010_388E; //  930×930 units,  entry on −Y axis

/// Spacing along road polylines (arnis blocks) between commercial placement intervals.
const COMMERCIAL_ROAD_SPACING: i32 = 14;

/// Setback from road edge to commercial building centre (arnis blocks).
const COMMERCIAL_SETBACK: i32 = 6;

/// Lamp post placed alongside roads.
const LAMP_POST_FID: u32 = 0x0003A74A;

/// Spacing along road polylines between consecutive lamp posts, in arnis blocks.
/// At scale 1.0 (1 block ≈ 1 m) this is one post roughly every 16 m.
const LAMP_SPACING_BLOCKS: i32 = 16;

/// Juniper — placed in LC_SHRUBLAND and LC_GRASSLAND cells.
const JUNIPER_FID: u32 = 0x001479E2;

/// Grid spacing for juniper placement.
const JUNIPER_GRID_SPACING: i32 = 8;

// ---------------------------------------------------------------------------
// Road rasterisation
// ---------------------------------------------------------------------------

/// Real-world half-width of a typical road in metres.
/// At scale 1.0 this is 3 arnis blocks each side of the centreline.
/// Below the scale at which this rounds to zero the road is omitted.
const ROAD_HALF_WIDTH_METERS: f64 = 3.0;

/// Returns the road half-width in arnis blocks for the given world scale,
/// or 0 if the road would be too narrow to paint (< 1 block each side).
fn road_half_width_blocks(world_scale: f64) -> i32 {
    (ROAD_HALF_WIDTH_METERS * world_scale).round() as i32
}

/// Flat boolean grid covering the arnis worldspace in block coordinates.
/// True = this block lies on or within `half_width` of a road centreline.
struct RoadGrid {
    data: Vec<bool>,
    width: i32,
    height: i32,
}

impl RoadGrid {
    fn new(width: i32, height: i32) -> Self {
        Self {
            data: vec![false; (width * height) as usize],
            width,
            height,
        }
    }

    #[inline]
    fn is_road(&self, x: i32, z: i32) -> bool {
        if x < 0 || z < 0 || x >= self.width || z >= self.height {
            return false;
        }
        self.data[(z * self.width + x) as usize]
    }

    #[inline]
    fn mark(&mut self, x: i32, z: i32) {
        if x < 0 || z < 0 || x >= self.width || z >= self.height {
            return;
        }
        self.data[(z * self.width + x) as usize] = true;
    }
}

/// Rasterise a set of road centreline polylines into a `RoadGrid`.
///
/// Each polyline is a sequence of arnis (x, z) coordinates. For every
/// segment the centreline is traced with Bresenham's line algorithm, then
/// expanded by `half_width` blocks in every direction to give the road its
/// painted width.
fn build_road_grid(
    polylines: &[(u8, Vec<(i32, i32)>)],
    world_width: i32,
    world_height: i32,
    half_width: i32,
) -> RoadGrid {
    let mut grid = RoadGrid::new(world_width, world_height);

    // Paint a line segment (already expanded by half_width) into the grid.
    let paint_segment = |grid: &mut RoadGrid, x0: i32, z0: i32, x1: i32, z1: i32| {
        for (cx, _, cz) in bresenham_line(x0, 0, z0, x1, 0, z1) {
            for dz in -half_width..=half_width {
                for dx in -half_width..=half_width {
                    grid.mark(cx + dx, cz + dz);
                }
            }
        }
    };

    for (_, polyline) in polylines {
        for segment in polyline.windows(2) {
            let (x0, z0) = segment[0];
            let (x1, z1) = segment[1];
            paint_segment(&mut grid, x0, z0, x1, z1);
        }
    }

    // Bridge small gaps between polyline endpoints that arise from independent
    // bbox-clip rounding.  Each polyline contributes two endpoints (first and
    // last node); if two endpoints from different polylines are within
    // (2*half_width + 2) blocks of each other we draw a connecting segment so
    // that the road appears continuous rather than broken.
    let gap_threshold = (2 * half_width + 2).max(4);
    let gap_threshold_sq = (gap_threshold * gap_threshold) as i64;

    // Collect (endpoint, polyline_index) pairs — first and last node of each polyline.
    let endpoints: Vec<(i32, i32, usize)> = polylines
        .iter()
        .enumerate()
        .flat_map(|(i, (_, pl))| {
            let mut ep = Vec::new();
            if let Some(&(x, z)) = pl.first() { ep.push((x, z, i)); }
            if pl.len() > 1 {
                if let Some(&(x, z)) = pl.last() { ep.push((x, z, i)); }
            }
            ep
        })
        .collect();

    for a in 0..endpoints.len() {
        for b in (a + 1)..endpoints.len() {
            let (ax, az, ai) = endpoints[a];
            let (bx, bz, bi) = endpoints[b];
            if ai == bi {
                continue; // same polyline — skip
            }
            let dx = (bx - ax) as i64;
            let dz = (bz - az) as i64;
            if dx * dx + dz * dz <= gap_threshold_sq {
                paint_segment(&mut grid, ax, az, bx, bz);
            }
        }
    }

    grid
}

// ---------------------------------------------------------------------------
// Per-cell occupancy grid
// ---------------------------------------------------------------------------

/// Per-cell occupancy bitmap that prevents overlapping object placement.
///
/// Before placing an object, call `is_clear(ax, az, radius)`. If it returns
/// true, place the object and then call `mark(ax, az, radius)` to claim the
/// space. Objects placed earlier in the call sequence take priority.
struct OccupancyGrid {
    data: Vec<bool>, // BLOCKS_PER_CELL × BLOCKS_PER_CELL, one bool per arnis block
    cell_min_ax: i32,
    cell_min_az: i32,
}

impl OccupancyGrid {
    fn new(cell_col: i32, cell_row: i32) -> Self {
        Self {
            data: vec![false; (BLOCKS_PER_CELL * BLOCKS_PER_CELL) as usize],
            cell_min_ax: cell_col * BLOCKS_PER_CELL,
            cell_min_az: cell_row * BLOCKS_PER_CELL,
        }
    }

    /// Returns true if every block within `radius` of `(ax, az)` is unoccupied.
    /// Positions outside this cell's boundary are treated as clear.
    fn is_clear(&self, ax: i32, az: i32, radius: i32) -> bool {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let lx = ax + dx - self.cell_min_ax;
                let lz = az + dz - self.cell_min_az;
                if lx >= 0
                    && lz >= 0
                    && lx < BLOCKS_PER_CELL
                    && lz < BLOCKS_PER_CELL
                    && self.data[(lz * BLOCKS_PER_CELL + lx) as usize]
                {
                    return false;
                }
            }
        }
        true
    }

    /// Mark all blocks within `radius` of `(ax, az)` as occupied.
    fn mark(&mut self, ax: i32, az: i32, radius: i32) {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let lx = ax + dx - self.cell_min_ax;
                let lz = az + dz - self.cell_min_az;
                if lx >= 0 && lz >= 0 && lx < BLOCKS_PER_CELL && lz < BLOCKS_PER_CELL {
                    self.data[(lz * BLOCKS_PER_CELL + lx) as usize] = true;
                }
            }
        }
    }
}

/// Exclusion radius (arnis blocks) for each object category.
/// A placed object claims a square of side `2*radius+1` centred on its position.
const OCC_RADIUS_BUILDING: i32 = 5; // houses (~5.8 blocks/side) and commercial
const OCC_RADIUS_TREE: i32 = 3;     // forest / desert trees
const OCC_RADIUS_SHRUB: i32 = 2;    // junipers
const OCC_RADIUS_ROCK: i32 = 2;     // rock clusters
const OCC_RADIUS_LAMP: i32 = 1;     // lamp posts (tiny footprint)

// --- binary helpers ---

#[inline]
fn pu16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn pu32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn pi16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn pi32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn pf32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

// --- record / GRUP writers ---

/// Write a 24-byte record header followed by `data` into `buf`.
fn push_record(buf: &mut Vec<u8>, tag: &[u8; 4], flags: u32, form_id: u32, data: &[u8]) {
    buf.extend_from_slice(tag);
    pu32(buf, data.len() as u32);
    pu32(buf, flags);
    pu32(buf, form_id);
    pu32(buf, 0); // VCS revision
    pu16(buf, FNV_FORM_VERSION);
    pu16(buf, 0); // vcinfo2
    buf.extend_from_slice(data);
}

/// Write a 24-byte FNV GRUP header followed by `content` into `buf`.
///
/// FNV uses a 24-byte GRUP header (4 bytes larger than Oblivion's 20-byte
/// layout).  The size field includes all 24 header bytes.
///
/// Layout: "GRUP"(4) + size(4) + label(4) + type(4) + stamp(2) + unk1(2) + unk2(4)
fn push_grup(buf: &mut Vec<u8>, label: [u8; 4], group_type: i32, content: &[u8]) {
    buf.extend_from_slice(b"GRUP");
    pu32(buf, (24 + content.len()) as u32);
    buf.extend_from_slice(&label);
    pi32(buf, group_type);
    pu16(buf, 0); // stamp
    pu16(buf, 0); // unk1
    pu32(buf, 0); // unk2 — extra 4 bytes present in FO3/FNV
    buf.extend_from_slice(content);
}

/// Pack two i16 coordinates into a 4-byte GRUP label (X first, then Y, LE).
fn xy_label(x: i16, y: i16) -> [u8; 4] {
    let mut label = [0u8; 4];
    label[0..2].copy_from_slice(&x.to_le_bytes());
    label[2..4].copy_from_slice(&y.to_le_bytes());
    label
}

/// Build a subrecord: 4-byte tag + u16 data-size + data bytes.
fn subrecord(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(tag);
    pu16(&mut buf, data.len() as u16);
    buf.extend_from_slice(data);
    buf
}

// --- VHGT delta encoding ---

/// Sample and smooth the 33×33 vertex height grid for one cell.
///
/// FNV VHGT convention:
///   row 0 = SOUTH edge of the cell (higher arnis Z, since Z increases south)
///   row 32 = NORTH edge of the cell (lower arnis Z)
///   col 0 = WEST edge, col 32 = EAST edge
///
/// The heights are sampled into a padded (VERTS + 2×SMOOTH_PAD)² grid that
/// extends SMOOTH_PAD vertices beyond each cell edge, then smoothed in-place
/// with SMOOTH_ITERS passes of a gradient-limited Laplacian filter.  Because
/// the padded region is sourced from the same ground.level() function as every
/// other cell, adjacent cells always compute identical heights at their shared
/// border vertices — no inter-cell seams are introduced by the smoothing.
fn sample_heights(
    ground: &Ground,
    cell_col: i32,
    cell_row: i32,
    global_min: i32,
    scale: i32,
) -> [[i32; VERTS]; VERTS] {
    const PSIZE: usize = VERTS + 2 * SMOOTH_PAD;

    // ── Sample padded grid ──────────────────────────────────────────────────
    let mut grid = [[0i32; PSIZE]; PSIZE];
    for pr in 0..PSIZE {
        for pc in 0..PSIZE {
            // Map padded indices back to arnis world coordinates.
            // pr=SMOOTH_PAD corresponds to grid row 0 (south edge, large arnis z).
            let x = cell_col * BLOCKS_PER_CELL + pc as i32 - SMOOTH_PAD as i32;
            let z = cell_row * BLOCKS_PER_CELL
                + VERTS as i32 - 1
                - (pr as i32 - SMOOTH_PAD as i32);
            let raw_h = ground.level(XZPoint::new(x, z));
            grid[pr][pc] = (raw_h - global_min) * scale + HEIGHT_MARGIN;
        }
    }

    // ── Iterative gradient-limited Laplacian smoothing ─────────────────────
    // Each pass: vertices with a steep neighbour gradient blend toward the
    // 4-neighbour average.  Blend strength scales linearly from zero at
    // SMOOTH_THRESHOLD up to SMOOTH_MAX_BLEND at SMOOTH_FULL.
    for _ in 0..SMOOTH_ITERS {
        let prev = grid; // [[i32; PSIZE]; PSIZE] is Copy — this is a stack copy
        for pr in 1..PSIZE - 1 {
            for pc in 1..PSIZE - 1 {
                let h = prev[pr][pc];
                let neighbours = [
                    prev[pr - 1][pc],
                    prev[pr + 1][pc],
                    prev[pr][pc - 1],
                    prev[pr][pc + 1],
                ];
                let max_grad = neighbours.iter().map(|&n| (h - n).abs()).max().unwrap();
                if max_grad > SMOOTH_THRESHOLD {
                    let avg: i32 = neighbours.iter().sum::<i32>() / 4;
                    let strength = ((max_grad - SMOOTH_THRESHOLD) as f32
                        / (SMOOTH_FULL - SMOOTH_THRESHOLD) as f32)
                        .clamp(0.0, 1.0)
                        * SMOOTH_MAX_BLEND;
                    grid[pr][pc] = h + ((avg - h) as f32 * strength).round() as i32;
                }
            }
        }
    }

    // ── Extract inner VERTS×VERTS ───────────────────────────────────────────
    let mut heights = [[0i32; VERTS]; VERTS];
    for row in 0..VERTS {
        for col in 0..VERTS {
            heights[row][col] = grid[row + SMOOTH_PAD][col + SMOOTH_PAD];
        }
    }
    heights
}

/// Encode a height grid as VHGT fOffset + delta bytes.
fn encode_vhgt(heights: &[[i32; VERTS]; VERTS]) -> (f32, Vec<i8>) {
    let vhgt_offset = heights[0][0] as f32;

    let mut deltas = Vec::with_capacity(VERTS * VERTS);
    deltas.push(0i8);
    for col in 1..VERTS {
        let d = heights[0][col] - heights[0][col - 1];
        deltas.push(d.clamp(-128, 127) as i8);
    }
    for row in 1..VERTS {
        let d0 = heights[row][0] - heights[row - 1][0];
        deltas.push(d0.clamp(-128, 127) as i8);
        for col in 1..VERTS {
            let d = heights[row][col] - heights[row][col - 1];
            deltas.push(d.clamp(-128, 127) as i8);
        }
    }
    (vhgt_offset, deltas)
}

/// Compute per-vertex surface normals from the scaled height grid.
///
/// FNV VNML component layout: (NX=east, NY=north, NZ=up) as signed bytes.
///
/// Each vertex normal is derived from the local terrain gradient using central
/// differences for interior vertices and one-sided differences at borders.
/// The "up" component uses the vertex spacing (128 game-units / 8 = 16 VHGT
/// units) so slopes are rendered at their true physical angle.
///
/// With the row-flip convention (row 0 = south, row 32 = north), increasing
/// row index = moving northward, so dh/d(row) is the north gradient.
fn compute_vnml(heights: &[[i32; VERTS]; VERTS]) -> Vec<u8> {
    // Horizontal vertex spacing in the same units as the height values:
    //   128 game-units between vertices / 8 game-units per VHGT unit = 16.
    const SPACING: f32 = 16.0;

    let mut vnml = Vec::with_capacity(VERTS * VERTS * 3);
    for row in 0..VERTS {
        for col in 0..VERTS {
            // East gradient (col increases east)
            let dh_east = if col == 0 {
                (heights[row][1] - heights[row][0]) as f32
            } else if col == VERTS - 1 {
                (heights[row][VERTS - 1] - heights[row][VERTS - 2]) as f32
            } else {
                (heights[row][col + 1] - heights[row][col - 1]) as f32 * 0.5
            };

            // North gradient (increasing row = north with the row-flip)
            let dh_north = if row == 0 {
                (heights[1][col] - heights[0][col]) as f32
            } else if row == VERTS - 1 {
                (heights[VERTS - 1][col] - heights[VERTS - 2][col]) as f32
            } else {
                (heights[row + 1][col] - heights[row - 1][col]) as f32 * 0.5
            };

            // Surface normal = (-dh/dx, -dh/dy, spacing), then normalize.
            let nx = -dh_east;
            let ny = -dh_north;
            let nz = SPACING;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();

            let pack = |v: f32| -> u8 {
                ((v / len * 127.0).round().clamp(-127.0, 127.0) as i8) as u8
            };
            vnml.push(pack(nx)); // east
            vnml.push(pack(ny)); // north
            vnml.push(pack(nz)); // up
        }
    }
    vnml
}

// --- texture helpers ---

/// Maps a single ESA WorldCover land-cover class to a terrain texture FormID.
#[inline]
fn texture_for_cover(lc: u8) -> u32 {
    match lc {
        land_cover::LC_SNOW_ICE => TEXTURE_SNOW,
        land_cover::LC_BUILT_UP => TEXTURE_ASPHALT,
        land_cover::LC_BARE => TEXTURE_SAND,
        land_cover::LC_CROPLAND | land_cover::LC_GRASSLAND | land_cover::LC_SHRUBLAND | land_cover::LC_TREE_COVER => TEXTURE_GRASS,
        _ => TEXTURE_DIRT,
        // _ => TEXTURE_GRASS,
    }
}

/// Per-vertex texture data for one quadrant of a LAND record.
struct QuadTexture {
    /// BTXT base FormID — the dominant texture in this quadrant.
    base_fid: u32,
    /// ATXT+VTXT layers for minority vertices: (FormID, sorted vertex entries).
    ///
    /// VTXT entry: (position in 17×17 quadrant grid 0–288, opacity 0.0–1.0).
    /// Only vertices whose texture differs from `base_fid` appear here.
    layers: Vec<(u32, Vec<(u16, f32)>)>,
}

/// Returns the local slope at `(row, col)` in a VERTS×VERTS height grid,
/// in VHGT units per vertex, using central differences for interior vertices
/// and one-sided differences at the grid border.
#[inline]
fn slope_at(heights: &[[i32; VERTS]; VERTS], row: usize, col: usize) -> i32 {
    let h = heights[row][col];
    let dh_row = if row == 0 {
        (heights[1][col] - h).abs()
    } else if row == VERTS - 1 {
        (h - heights[VERTS - 2][col]).abs()
    } else {
        (heights[row + 1][col] - heights[row - 1][col]).abs() / 2
    };
    let dh_col = if col == 0 {
        (heights[row][1] - h).abs()
    } else if col == VERTS - 1 {
        (h - heights[row][VERTS - 2]).abs()
    } else {
        (heights[row][col + 1] - heights[row][col - 1]).abs() / 2
    };
    dh_row.max(dh_col)
}

/// Assign per-vertex textures and build BTXT/ATXT/VTXT data for all four
/// quadrants of one LAND record.
///
/// Each vertex receives exactly one texture, chosen by priority:
///   1. Road grid hit        → TEXTURE_ASPHALT
///   2. Steep slope          → TEXTURE_DIRT  (cliff / exposed rock)
///   3. Land cover class     → texture_for_cover(lc)
///   4. Fallback             → default_texture
///
/// Texture boundaries therefore follow terrain contours (steep slopes) and
/// road edges rather than invisible land-cover class lines.  No neighbourhood
/// blending is performed; opacity is always 1.0 for non-base vertices.
///
/// A single cell-wide BTXT (the most common texture across all four quadrants)
/// is used for every quadrant, preventing the mid-cell seam that would appear
/// if adjacent quadrants had different base textures.
///
/// Quadrant layout in the 33×33 LAND vertex grid (row 0 = south):
///   SW (0): rows  0–16, cols  0–16
///   SE (1): rows  0–16, cols 16–32
///   NW (2): rows 16–32, cols  0–16
///   NE (3): rows 16–32, cols 16–32
///
/// FNV quad indices: 0=SW, 1=SE, 2=NW, 3=NE.  Vertex 0 within each quad is
/// the SW corner of that quad (row_start, col_start), with V increasing
/// northward and C increasing eastward.
fn compute_quad_textures(
    ground: &Ground,
    cell_col: usize,
    cell_row: usize,
    heights: &[[i32; VERTS]; VERTS],
    default_texture: u32,
    road_grid: Option<&RoadGrid>,
) -> [QuadTexture; 4] {
    // (land_row_start, land_col_start) for FNV quadrants SW=0, SE=1, NW=2, NE=3
    const QUAD_ORIGIN: [(usize, usize); 4] = [(0, 0), (0, 16), (16, 0), (16, 16)];

    // VHGT gradient threshold above which a vertex is treated as a steep slope
    // and receives the rocky/dirt texture.
    // 12 units ≈ atan(96 / 128) ≈ 37° — visible cliff faces get TEXTURE_DIRT.
    const SLOPE_ROCKY: i32 = 12;

    // ── Pass 1: assign one texture per vertex, tally cell-wide counts ────────
    let mut all_vertex_tex: [Vec<u32>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut cell_counts: BTreeMap<u32, u32> = BTreeMap::new();

    for quad in 0..4usize {
        let (row_start, col_start) = QUAD_ORIGIN[quad];
        let vt = &mut all_vertex_tex[quad];
        vt.reserve(17 * 17);

        for qr in 0..17usize {
            for qc in 0..17usize {
                let land_row = row_start + qr;
                let land_col = col_start + qc;
                let ax = cell_col as i32 * BLOCKS_PER_CELL + land_col as i32;
                let az = cell_row as i32 * BLOCKS_PER_CELL + (VERTS - 1 - land_row) as i32;

                let tex = if road_grid.map_or(false, |rg| rg.is_road(ax, az)) {
                    TEXTURE_ASPHALT
                } else if slope_at(heights, land_row, land_col) >= SLOPE_ROCKY {
                    TEXTURE_DIRT
                } else {
                    let lc = ground.cover_class(XZPoint::new(ax, az));
                    if lc == 0 {
                        default_texture
                    } else {
                        texture_for_cover(lc)
                    }
                };

                vt.push(tex);
                *cell_counts.entry(tex).or_insert(0) += 1;
            }
        }
    }

    // ── Cell-wide base texture ───────────────────────────────────────────────
    let cell_base_fid = cell_counts
        .iter()
        .max_by_key(|(_, &count)| count)
        .map(|(&fid, _)| fid)
        .unwrap_or(TEXTURE_DIRT);

    // ── Pass 2: build ATXT/VTXT layers for non-base vertices (opacity 1.0) ───
    let quads: Vec<QuadTexture> = all_vertex_tex
        .into_iter()
        .map(|vertex_tex| {
            let mut extra: BTreeMap<u32, Vec<(u16, f32)>> = BTreeMap::new();
            for (i, &tex) in vertex_tex.iter().enumerate() {
                if tex != cell_base_fid {
                    extra.entry(tex).or_default().push((i as u16, 1.0));
                }
            }
            QuadTexture {
                base_fid: cell_base_fid,
                layers: extra.into_iter().collect(),
            }
        })
        .collect();

    quads.try_into().unwrap_or_else(|_| unreachable!("always 4 quadrants"))
}

// --- record builders ---

/// `min/max_cell_x/y` are the actual FNV cell grid coordinates (centered on 0,0).
fn build_wrld_record(
    min_cell_x: i32,
    max_cell_x: i32,
    min_cell_y: i32,
    max_cell_y: i32,
    water_height_game: Option<f32>,
) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend(subrecord(b"EDID", b"ArnisWorldspace\0"));
    data.extend(subrecord(b"FULL", b"Arnis Worldspace\0"));

    // NAM2: ambient music (use same ref as testesm — 0x00000018 in FalloutNV.esm)
    let mut nam2 = Vec::new();
    pu32(&mut nam2, 0x00000018u32);
    data.extend(subrecord(b"NAM2", &nam2));

    // NAM3: encounter zone (null = none)
    let mut nam3 = Vec::new();
    pu32(&mut nam3, 0x00000000u32);
    data.extend(subrecord(b"NAM3", &nam3));

    // NAM4: climate override (null = default)
    let mut nam4 = Vec::new();
    pu32(&mut nam4, 0x00000000u32);
    data.extend(subrecord(b"NAM4", &nam4));

    // DNAM: default land height + default water height
    let mut dnam = Vec::new();
    pf32(&mut dnam, 0.0_f32); // defaultLandHeight
    pf32(&mut dnam, water_height_game.unwrap_or(0.0_f32)); // defaultWaterHeight
    data.extend(subrecord(b"DNAM", &dnam));

    // MNAM: usable map image dims + NW cell coords + SE cell coords.
    // NW = min X, max Y (north-west corner); SE = max X, min Y.
    let mut mnam = Vec::new();
    pi32(&mut mnam, 0); // usable width (0 = no map image)
    pi32(&mut mnam, 0); // usable height
    pi16(&mut mnam, min_cell_x as i16); // NW cell X (westernmost)
    pi16(&mut mnam, max_cell_y as i16); // NW cell Y (northernmost)
    pi16(&mut mnam, max_cell_x as i16); // SE cell X (easternmost)
    pi16(&mut mnam, min_cell_y as i16); // SE cell Y (southernmost)
    data.extend(subrecord(b"MNAM", &mnam));

    // ONAM: map scale + camera offsets
    let mut onam = Vec::new();
    pf32(&mut onam, 1.0_f32);
    pf32(&mut onam, 0.0_f32);
    pf32(&mut onam, 0.0_f32);
    data.extend(subrecord(b"ONAM", &onam));

    // DATA: 0x01 = Small World flag
    data.extend(subrecord(b"DATA", &[0x01u8]));

    // NAM0 / NAM9: world bounds in game units (SW corner, NE corner).
    let mut nam0 = Vec::new();
    pf32(&mut nam0, min_cell_x as f32 * CELL_GAME_UNITS);
    pf32(&mut nam0, min_cell_y as f32 * CELL_GAME_UNITS);
    data.extend(subrecord(b"NAM0", &nam0));

    let mut nam9 = Vec::new();
    pf32(&mut nam9, (max_cell_x + 1) as f32 * CELL_GAME_UNITS);
    pf32(&mut nam9, (max_cell_y + 1) as f32 * CELL_GAME_UNITS);
    data.extend(subrecord(b"NAM9", &nam9));

    // NNAM / XNAM: required flags present in all working FNV worldspace records
    data.extend(subrecord(b"NNAM", &[0x00u8]));
    data.extend(subrecord(b"XNAM", &[0x00u8]));

    let mut buf = Vec::new();
    push_record(&mut buf, b"WRLD", 0, TESTESM_WRLD_FID, &data);
    buf
}

fn build_cell_record(
    form_id: u32,
    cell_x: i32,
    cell_y: i32,
    water_height_game: Option<f32>,
) -> Vec<u8> {
    let mut data = Vec::new();
    // DATA flag 0x02 = HasWater — required for FNV to render water in this exterior cell.
    let cell_flags: u8 = if water_height_game.is_some() { 0x02 } else { 0x00 };
    data.extend(subrecord(b"DATA", &[cell_flags]));

    let mut xclc = Vec::new();
    pi32(&mut xclc, cell_x);
    pi32(&mut xclc, cell_y);
    pu32(&mut xclc, 0u32);
    data.extend(subrecord(b"XCLC", &xclc));

    // XCLW: per-cell water height override (game units). Required when DATA has 0x02.
    if let Some(wh) = water_height_game {
        let mut xclw = Vec::new();
        pf32(&mut xclw, wh);
        data.extend(subrecord(b"XCLW", &xclw));
    }

    let mut buf = Vec::new();
    push_record(&mut buf, b"CELL", 0, form_id, &data);
    buf
}

/// Build a LAND record with per-vertex texture painting.
///
/// Subrecord order matches the reference painted cell:
///   DATA → VNML → VHGT → VCLR → per-quadrant(BTXT [ATXT VTXT]*)
///
/// DATA flags 0x1F: VHGT(0x01) | VNML(0x02) | world-map(0x04) | VCLR(0x08) | textures(0x10).
///
/// For each quadrant the dominant texture is written as BTXT (the base that
/// covers the whole quadrant). Vertices with a different texture get an
/// ATXT record (with an incrementing layer index) followed by VTXT entries
/// at opacity 1.0, fully overriding the base at those positions.
///
/// BTXT layout (8 bytes): FormID(4) | quad(1) | zero(1) | layer=-1(i16 LE)
/// ATXT layout (8 bytes): FormID(4) | quad(1) | zero(1) | layer(u16 LE)
/// VTXT entry (8 bytes):  pos(u16 LE) | 0xFFFF(u16) | opacity(f32 LE)
fn build_land_record(
    form_id: u32,
    vhgt_offset: f32,
    deltas: &[i8],
    vnml: &[u8],
    quads: &[QuadTexture; 4],
) -> Vec<u8> {
    let mut flags_bytes = Vec::new();
    pu32(&mut flags_bytes, 0x0000_001Fu32);

    let mut vhgt = Vec::with_capacity(1096);
    pf32(&mut vhgt, vhgt_offset);
    for &d in deltas {
        vhgt.push(d as u8);
    }
    vhgt.extend_from_slice(&[0u8, 0u8, 0u8]);

    let mut data = Vec::new();
    data.extend(subrecord(b"DATA", &flags_bytes));
    data.extend(subrecord(b"VNML", vnml));
    data.extend(subrecord(b"VHGT", &vhgt));

    // VCLR: neutral mid-grey vertex colors (no tinting).
    let vclr = vec![128u8; VERTS * VERTS * 3];
    data.extend(subrecord(b"VCLR", &vclr));

    // Texture subrecords — one BTXT per quadrant, then ATXT+VTXT for minority vertices.
    for (quad_idx, qt) in quads.iter().enumerate() {
        let quad = quad_idx as u8;

        // BTXT: base texture for this quadrant
        let mut btxt = Vec::new();
        pu32(&mut btxt, qt.base_fid);
        btxt.push(quad);
        btxt.push(0u8);
        pi16(&mut btxt, -1); // layer = 0xFFFF
        data.extend(subrecord(b"BTXT", &btxt));

        // ATXT + VTXT: additional layers for vertices that differ from the base
        for (layer_idx, (tex_fid, vtxt_entries)) in qt.layers.iter().enumerate() {
            let mut atxt = Vec::new();
            pu32(&mut atxt, *tex_fid);
            atxt.push(quad);
            atxt.push(0u8);
            pu16(&mut atxt, layer_idx as u16);
            data.extend(subrecord(b"ATXT", &atxt));

            let mut vtxt = Vec::new();
            for &(pos, opacity) in vtxt_entries {
                pu16(&mut vtxt, pos);
                pu16(&mut vtxt, 0xFFFF); // unknown, always 0xFFFF in reference data
                pf32(&mut vtxt, opacity);
            }
            data.extend(subrecord(b"VTXT", &vtxt));
        }
    }

    let mut buf = Vec::new();
    push_record(&mut buf, b"LAND", 0, form_id, &data);
    buf
}

// --- vegetation helpers ---

/// Build a single REFR record for a static object placed in the world.
///
/// `form_id`  — the FormID to assign this reference record.
/// `base_fid` — the base object FormID (e.g. JUNIPER_FID).
/// `x/y/z`   — FNV game-unit position (X east, Y north, Z up).
/// `rot_z`   — rotation around the Z axis in radians (for variety).
fn build_tree_refr(form_id: u32, base_fid: u32, x: f32, y: f32, z: f32, rot_z: f32) -> Vec<u8> {
    let mut data = Vec::new();

    // NAME: base object FormID
    let mut name_data = Vec::new();
    pu32(&mut name_data, base_fid);
    data.extend(subrecord(b"NAME", &name_data));

    // DATA: position (XYZ) + rotation (XYZ) as f32s
    let mut pos_data = Vec::new();
    pf32(&mut pos_data, x);
    pf32(&mut pos_data, y);
    pf32(&mut pos_data, z);
    pf32(&mut pos_data, 0.0); // rot X
    pf32(&mut pos_data, 0.0); // rot Y
    pf32(&mut pos_data, rot_z);
    data.extend(subrecord(b"DATA", &pos_data));

    let mut buf = Vec::new();
    push_record(&mut buf, b"REFR", 0, form_id, &data);
    buf
}

/// Generate REFR records for trees in one cell.
///
/// Iterates a regular grid across the cell at `TREE_GRID_SPACING`-block
/// intervals. At each grid point, samples the land-cover class and places:
///   - LC_TREE_COVER → Cedar or Aspen (random 50/50, ~60% density)
///   - LC_BARE       → Joshua Tree or Palm (random 50/50, ~20% density)
/// A deterministic per-coordinate RNG decides placement and adds positional/
/// rotational jitter.
///
/// Returns the concatenated REFR byte data and the number of records written.
/// `next_fid` is advanced by the returned count.
fn place_tree_refs(
    ground: &Ground,
    cell_col: i32,
    cell_row: i32,
    cell_x: i32,
    cell_y: i32,
    global_min: i32,
    effective_scale: i32,
    water_height_game: Option<f32>,
    next_fid: &mut u32,
    occ: &mut OccupancyGrid,
) -> (Vec<u8>, u32) {
    let mut refs = Vec::new();
    let mut count = 0u32;

    let mut local_row_fnv = 0i32;
    while local_row_fnv < BLOCKS_PER_CELL {
        let mut local_col = 0i32;
        while local_col < BLOCKS_PER_CELL {
            // arnis coords: Z increases southward; FNV row 0 = south = large arnis Z.
            let arnis_x = cell_col * BLOCKS_PER_CELL + local_col;
            let arnis_z = cell_row * BLOCKS_PER_CELL + (BLOCKS_PER_CELL - 1 - local_row_fnv);

            let lc = ground.cover_class(XZPoint::new(arnis_x, arnis_z));
            let place_prob = match lc {
                land_cover::LC_TREE_COVER => 0.6,
                land_cover::LC_BARE       => 0.2,
                _                         => 0.0,
            };
            if place_prob > 0.0 {
                let mut rng = coord_rng(arnis_x, arnis_z, 0x54524545_00000000);

                if rng.random_bool(place_prob) {
                    let tree_fid = match lc {
                        land_cover::LC_TREE_COVER => {
                            if rng.random_bool(0.5) { CEDAR_FID } else { ASPEN_FID }
                        }
                        _ => {
                            if rng.random_bool(0.5) { JOSHUA_TREE_FID } else { PALM_TREE_FID }
                        }
                    };

                    let jitter_x: f32 = rng.random_range(-48.0_f32..48.0_f32);
                    let jitter_y: f32 = rng.random_range(-48.0_f32..48.0_f32);
                    let rot_z: f32 = rng.random_range(0.0_f32..std::f32::consts::TAU);

                    let game_x = cell_x as f32 * CELL_GAME_UNITS
                        + local_col as f32 * 128.0
                        + 64.0
                        + jitter_x;
                    let game_y = cell_y as f32 * CELL_GAME_UNITS
                        + local_row_fnv as f32 * 128.0
                        + 64.0
                        + jitter_y;

                    let raw_h = ground.level(XZPoint::new(arnis_x, arnis_z));
                    let game_z =
                        ((raw_h - global_min) * effective_scale + HEIGHT_MARGIN) as f32 * 8.0;

                    if water_height_game.map_or(true, |wl| game_z >= wl)
                        && occ.is_clear(arnis_x, arnis_z, OCC_RADIUS_TREE)
                    {
                        refs.extend(build_tree_refr(*next_fid, tree_fid, game_x, game_y, game_z, rot_z));
                        *next_fid += 1;
                        count += 1;
                        occ.mark(arnis_x, arnis_z, OCC_RADIUS_TREE);
                    }
                }
            }

            local_col += TREE_GRID_SPACING;
        }
        local_row_fnv += TREE_GRID_SPACING;
    }

    (refs, count)
}

/// Generate REFR records for rocks in one cell.
///
/// Rocks are placed in `LC_BARE` terrain at a high density and in
/// `LC_SHRUBLAND` at a reduced density (rocky outcrops among brush).
/// Rock type is chosen by weighted RNG — smaller variants are more common.
/// All four variants look fine from any angle, so rotation is fully random.
fn place_rock_refs(
    ground: &Ground,
    cell_col: i32,
    cell_row: i32,
    cell_x: i32,
    cell_y: i32,
    global_min: i32,
    effective_scale: i32,
    water_height_game: Option<f32>,
    next_fid: &mut u32,
    occ: &mut OccupancyGrid,
) -> (Vec<u8>, u32) {
    let mut refs = Vec::new();
    let mut count = 0u32;

    let mut local_row_fnv = 0i32;
    while local_row_fnv < BLOCKS_PER_CELL {
        let mut local_col = 0i32;
        while local_col < BLOCKS_PER_CELL {
            let arnis_x = cell_col * BLOCKS_PER_CELL + local_col;
            let arnis_z = cell_row * BLOCKS_PER_CELL + (BLOCKS_PER_CELL - 1 - local_row_fnv);

            let lc = ground.cover_class(XZPoint::new(arnis_x, arnis_z));

            // Placement probability: bare land ~65%, shrubland ~20% (rocky outcrops).
            let place_prob = match lc {
                land_cover::LC_BARE => 0.65,
                land_cover::LC_SHRUBLAND => 0.20,
                _ => 0.0,
            };

            if place_prob > 0.0 {
                let mut rng = coord_rng(arnis_x, arnis_z, 0x524F434B_53000000);

                if rng.random_bool(place_prob) {
                    // Weighted rock type: 40% small cluster, 30% tall, 20% medium, 10% large.
                    let rock_fid = match rng.random_range(0u32..10) {
                        0..=3 => ROCK_SMALL_CLUSTER_FID,
                        4..=6 => ROCK_TALL_FID,
                        7..=8 => ROCK_MEDIUM_FID,
                        _     => ROCK_LARGE_FID,
                    };

                    // Small jitter so rocks don't sit on a perfect grid.
                    let jitter_x: f32 = rng.random_range(-40.0_f32..40.0_f32);
                    let jitter_y: f32 = rng.random_range(-40.0_f32..40.0_f32);
                    let rot_z: f32 = rng.random_range(0.0_f32..std::f32::consts::TAU);

                    let game_x = cell_x as f32 * CELL_GAME_UNITS
                        + local_col as f32 * 128.0
                        + 64.0
                        + jitter_x;
                    let game_y = cell_y as f32 * CELL_GAME_UNITS
                        + local_row_fnv as f32 * 128.0
                        + 64.0
                        + jitter_y;

                    let raw_h = ground.level(XZPoint::new(arnis_x, arnis_z));
                    let game_z =
                        ((raw_h - global_min) * effective_scale + HEIGHT_MARGIN) as f32 * 8.0;

                    if water_height_game.map_or(true, |wl| game_z >= wl)
                        && occ.is_clear(arnis_x, arnis_z, OCC_RADIUS_ROCK)
                    {
                        refs.extend(build_tree_refr(*next_fid, rock_fid, game_x, game_y, game_z, rot_z));
                        *next_fid += 1;
                        count += 1;
                        occ.mark(arnis_x, arnis_z, OCC_RADIUS_ROCK);
                    }
                }
            }

            local_col += ROCK_GRID_SPACING;
        }
        local_row_fnv += ROCK_GRID_SPACING;
    }

    (refs, count)
}

/// Generate REFR records for Juniper trees in one cell.
///
/// Dense in `LC_SHRUBLAND`, sparser in `LC_GRASSLAND`.  Rotation is fully
/// random — the asset looks fine from any angle.
fn place_shrub_refs(
    ground: &Ground,
    cell_col: i32,
    cell_row: i32,
    cell_x: i32,
    cell_y: i32,
    global_min: i32,
    effective_scale: i32,
    water_height_game: Option<f32>,
    next_fid: &mut u32,
    occ: &mut OccupancyGrid,
) -> (Vec<u8>, u32) {
    let mut refs = Vec::new();
    let mut count = 0u32;

    let mut local_row_fnv = 0i32;
    while local_row_fnv < BLOCKS_PER_CELL {
        let mut local_col = 0i32;
        while local_col < BLOCKS_PER_CELL {
            let arnis_x = cell_col * BLOCKS_PER_CELL + local_col;
            let arnis_z = cell_row * BLOCKS_PER_CELL + (BLOCKS_PER_CELL - 1 - local_row_fnv);

            let lc = ground.cover_class(XZPoint::new(arnis_x, arnis_z));

            let place_prob = match lc {
                land_cover::LC_SHRUBLAND => 0.70,
                land_cover::LC_GRASSLAND => 0.35,
                _ => 0.0,
            };

            if place_prob > 0.0 {
                let mut rng = coord_rng(arnis_x, arnis_z, 0x4A554E49_50455200);

                if rng.random_bool(place_prob) {
                    let jitter_x: f32 = rng.random_range(-40.0_f32..40.0_f32);
                    let jitter_y: f32 = rng.random_range(-40.0_f32..40.0_f32);
                    let rot_z: f32 = rng.random_range(0.0_f32..std::f32::consts::TAU);

                    let game_x = cell_x as f32 * CELL_GAME_UNITS
                        + local_col as f32 * 128.0
                        + 64.0
                        + jitter_x;
                    let game_y = cell_y as f32 * CELL_GAME_UNITS
                        + local_row_fnv as f32 * 128.0
                        + 64.0
                        + jitter_y;

                    let raw_h = ground.level(XZPoint::new(arnis_x, arnis_z));
                    let game_z =
                        ((raw_h - global_min) * effective_scale + HEIGHT_MARGIN) as f32 * 8.0;

                    if water_height_game.map_or(true, |wl| game_z >= wl)
                        && occ.is_clear(arnis_x, arnis_z, OCC_RADIUS_SHRUB)
                    {
                        refs.extend(build_tree_refr(*next_fid, JUNIPER_FID, game_x, game_y, game_z, rot_z));
                        *next_fid += 1;
                        count += 1;
                        occ.mark(arnis_x, arnis_z, OCC_RADIUS_SHRUB);
                    }
                }
            }

            local_col += JUNIPER_GRID_SPACING;
        }
        local_row_fnv += JUNIPER_GRID_SPACING;
    }

    (refs, count)
}

/// Generate REFR records for houses in one cell.
///
/// Walks every road polyline and at each `HOUSE_ROAD_SPACING`-block interval
/// attempts to place a house on both sides at `road_half_width + HOUSE_SETBACK`
/// blocks from the centreline.  This produces even street frontage regardless
/// of patchy land-cover data.
///
/// Only water and snow-ice land cover classes are hard disqualifiers; all other
/// terrain (grassland, built-up, cropland, etc.) is allowed.
///
/// Rotation: the house front is aimed toward the road centreline.
///   toward_fnv = (side × sdz/len,  side × sdx/len)   [arnis Z → FNV −Y]
///   House1 (default front = −X): rot_z = atan2(−toward_y, −toward_x)
///   House2/3 (default front = −Y): rot_z = atan2( toward_x, −toward_y)
fn place_house_refs(
    roads: &[(u8, Vec<(i32, i32)>)],
    road_grid: Option<&RoadGrid>,
    road_half_width: i32,
    world_scale: f64,
    cell_col: i32,
    cell_row: i32,
    cell_x: i32,
    cell_y: i32,
    ground: &Ground,
    global_min: i32,
    effective_scale: i32,
    water_height_game: Option<f32>,
    next_fid: &mut u32,
    occ: &mut OccupancyGrid,
) -> (Vec<u8>, u32) {
    let mut refs = Vec::new();
    let mut count = 0u32;

    if roads.is_empty() {
        return (refs, count);
    }

    let cell_min_ax = cell_col * BLOCKS_PER_CELL;
    let cell_max_ax = cell_min_ax + BLOCKS_PER_CELL;
    let cell_min_az = cell_row * BLOCKS_PER_CELL;
    let cell_max_az = cell_min_az + BLOCKS_PER_CELL;

    let setback = road_half_width as f64 + HOUSE_SETBACK as f64;

    // Scale-dependent keep probability, mirroring the lamp-post thinning.
    //   p(s) = clamp(0.70 × s^1.89, 0, 0.70)
    //   s = 0.142 → p ≈ 0.018  (~97.5 % fewer than scale 1)
    //   s = 1.00  → p = 0.70   (baseline)
    //   s ≥ 1.00  → p = 0.70   (capped)
    let keep_prob = (0.70_f64 * world_scale.powf(1.89)).clamp(0.0, 0.70);

    for (priority, polyline) in roads {
        if *priority != 1 {
            continue; // houses only on residential/local roads
        }
        for segment in polyline.windows(2) {
            let (x0, z0) = (segment[0].0 as f64, segment[0].1 as f64);
            let (x1, z1) = (segment[1].0 as f64, segment[1].1 as f64);
            let sdx = x1 - x0;
            let sdz = z1 - z0;
            let seg_len = (sdx * sdx + sdz * sdz).sqrt();
            if seg_len < 1.0 {
                continue;
            }

            // Perpendicular unit vector (left of travel direction in arnis XZ).
            let perp_x = -sdz / seg_len;
            let perp_z = sdx / seg_len;

            let num_steps = (seg_len / HOUSE_ROAD_SPACING as f64).ceil() as i32;

            for step in 0..=num_steps {
                let t = (step as f64 * HOUSE_ROAD_SPACING as f64).min(seg_len) / seg_len;
                let cx = x0 + sdx * t;
                let cz = z0 + sdz * t;

                for &side in &[1.0f64, -1.0f64] {
                    let px = (cx + side * perp_x * setback).round() as i32;
                    let pz = (cz + side * perp_z * setback).round() as i32;

                    if px < cell_min_ax || px >= cell_max_ax
                        || pz < cell_min_az || pz >= cell_max_az
                    {
                        continue;
                    }

                    // Skip positions on a road surface.
                    if road_grid.map_or(false, |rg| rg.is_road(px, pz)) {
                        continue;
                    }

                    // Hard disqualifiers: water and snow/ice only.
                    let lc = ground.cover_class(XZPoint::new(px, pz));
                    if lc == land_cover::LC_WATER || lc == land_cover::LC_SNOW_ICE {
                        continue;
                    }

                    // Scale-dependent density thinning.
                    let mut rng = coord_rng(px, pz, 0x484F555345_000000u64);
                    if !rng.random_bool(keep_prob) {
                        continue;
                    }

                    let house_fid = match rng.random_range(0u32..3) {
                        0 => HOUSE1_FID,
                        1 => HOUSE2_FID,
                        _ => HOUSE3_FID,
                    };

                    // "Toward road" unit vector in FNV XY (arnis Z → FNV −Y).
                    let toward_fnv_x = side * sdz / seg_len;
                    let toward_fnv_y = side * sdx / seg_len;

                    let rot_z = if house_fid == HOUSE1_FID {
                        f64::atan2(-toward_fnv_y, -toward_fnv_x) as f32
                    } else {
                        f64::atan2(toward_fnv_x, -toward_fnv_y) as f32
                    };

                    let local_col = px - cell_min_ax;
                    let local_row_fnv = BLOCKS_PER_CELL - 1 - (pz - cell_min_az);

                    let game_x = cell_x as f32 * CELL_GAME_UNITS
                        + local_col as f32 * 128.0 + 64.0;
                    let game_y = cell_y as f32 * CELL_GAME_UNITS
                        + local_row_fnv as f32 * 128.0 + 64.0;

                    let raw_h = ground.level(XZPoint::new(px, pz));
                    let game_z =
                        ((raw_h - global_min) * effective_scale + HEIGHT_MARGIN) as f32 * 8.0;

                    if water_height_game.map_or(true, |wl| game_z >= wl)
                        && occ.is_clear(px, pz, OCC_RADIUS_BUILDING)
                    {
                        refs.extend(build_tree_refr(
                            *next_fid, house_fid, game_x, game_y, game_z, rot_z,
                        ));
                        *next_fid += 1;
                        count += 1;
                        occ.mark(px, pz, OCC_RADIUS_BUILDING);
                    }
                }
            }
        }
    }

    (refs, count)
}

/// Generate REFR records for commercial buildings alongside higher-priority roads.
///
/// Walks road polylines with priority ≥ 2 (secondary, primary, trunk, motorway)
/// and at each `COMMERCIAL_ROAD_SPACING` interval places a building on both sides.
///
/// Rotation per model default-front axis:
///   COMM_2STORY / COMM_4STORY (entry −Y): rot_z = atan2( toward_x, −toward_y)
///   COMM_1STORY               (entry +Y): rot_z = atan2(−toward_x,  toward_y)
fn place_commercial_refs(
    roads: &[(u8, Vec<(i32, i32)>)],
    road_grid: Option<&RoadGrid>,
    road_half_width: i32,
    world_scale: f64,
    cell_col: i32,
    cell_row: i32,
    cell_x: i32,
    cell_y: i32,
    ground: &Ground,
    global_min: i32,
    effective_scale: i32,
    water_height_game: Option<f32>,
    next_fid: &mut u32,
    occ: &mut OccupancyGrid,
) -> (Vec<u8>, u32) {
    let mut refs = Vec::new();
    let mut count = 0u32;

    if roads.is_empty() {
        return (refs, count);
    }

    let cell_min_ax = cell_col * BLOCKS_PER_CELL;
    let cell_max_ax = cell_min_ax + BLOCKS_PER_CELL;
    let cell_min_az = cell_row * BLOCKS_PER_CELL;
    let cell_max_az = cell_min_az + BLOCKS_PER_CELL;

    let setback = road_half_width as f64 + COMMERCIAL_SETBACK as f64;

    //   p(s) = clamp(0.60 × s^1.53, 0, 0.60)
    //   s = 0.142 → p ≈ 0.030  (~95 % fewer than scale 1)
    //   s = 1.00  → p = 0.60   (baseline)
    //   s ≥ 1.00  → p = 0.60   (capped)
    let keep_prob = (0.60_f64 * world_scale.powf(1.53)).clamp(0.0, 0.60);

    for (priority, polyline) in roads {
        if *priority < 2 {
            continue; // commercial buildings only on main roads
        }
        for segment in polyline.windows(2) {
            let (x0, z0) = (segment[0].0 as f64, segment[0].1 as f64);
            let (x1, z1) = (segment[1].0 as f64, segment[1].1 as f64);
            let sdx = x1 - x0;
            let sdz = z1 - z0;
            let seg_len = (sdx * sdx + sdz * sdz).sqrt();
            if seg_len < 1.0 {
                continue;
            }

            let perp_x = -sdz / seg_len;
            let perp_z = sdx / seg_len;

            let num_steps = (seg_len / COMMERCIAL_ROAD_SPACING as f64).ceil() as i32;

            for step in 0..=num_steps {
                let t = (step as f64 * COMMERCIAL_ROAD_SPACING as f64).min(seg_len) / seg_len;
                let cx = x0 + sdx * t;
                let cz = z0 + sdz * t;

                for &side in &[1.0f64, -1.0f64] {
                    let px = (cx + side * perp_x * setback).round() as i32;
                    let pz = (cz + side * perp_z * setback).round() as i32;

                    if px < cell_min_ax || px >= cell_max_ax
                        || pz < cell_min_az || pz >= cell_max_az
                    {
                        continue;
                    }

                    if road_grid.map_or(false, |rg| rg.is_road(px, pz)) {
                        continue;
                    }

                    let lc = ground.cover_class(XZPoint::new(px, pz));
                    if lc == land_cover::LC_WATER || lc == land_cover::LC_SNOW_ICE {
                        continue;
                    }

                    let mut rng = coord_rng(px, pz, 0x434F4D4D_00000000u64);
                    if !rng.random_bool(keep_prob) {
                        continue;
                    }

                    let building_fid = match rng.random_range(0u32..3) {
                        0 => COMM_2STORY_FID,
                        1 => COMM_1STORY_FID,
                        _ => COMM_4STORY_FID,
                    };

                    // toward_road in FNV XY: (side×sdz/len, side×sdx/len)
                    let toward_fnv_x = side * sdz / seg_len;
                    let toward_fnv_y = side * sdx / seg_len;

                    let rot_z = if building_fid == COMM_1STORY_FID {
                        // entry on +Y axis
                        f64::atan2(-toward_fnv_x, toward_fnv_y) as f32
                    } else {
                        // entry on −Y axis (2-story and 4-story)
                        f64::atan2(toward_fnv_x, -toward_fnv_y) as f32
                    };

                    let local_col = px - cell_min_ax;
                    let local_row_fnv = BLOCKS_PER_CELL - 1 - (pz - cell_min_az);

                    let game_x = cell_x as f32 * CELL_GAME_UNITS
                        + local_col as f32 * 128.0 + 64.0;
                    let game_y = cell_y as f32 * CELL_GAME_UNITS
                        + local_row_fnv as f32 * 128.0 + 64.0;

                    let raw_h = ground.level(XZPoint::new(px, pz));
                    let game_z =
                        ((raw_h - global_min) * effective_scale + HEIGHT_MARGIN) as f32 * 8.0;

                    if water_height_game.map_or(true, |wl| game_z >= wl)
                        && occ.is_clear(px, pz, OCC_RADIUS_BUILDING)
                    {
                        refs.extend(build_tree_refr(
                            *next_fid, building_fid, game_x, game_y, game_z, rot_z,
                        ));
                        *next_fid += 1;
                        count += 1;
                        occ.mark(px, pz, OCC_RADIUS_BUILDING);
                    }
                }
            }
        }
    }

    (refs, count)
}

/// Generate REFR records for lamp posts alongside roads in one cell.
///
/// Walks every road polyline segment and places a lamp post at each
/// `LAMP_SPACING_BLOCKS` interval, offset perpendicularly from the road
/// centreline by `road_half_width + 2` blocks (just outside the road edge).
/// Posts falling outside this cell's arnis bounds are skipped, so each cell
/// independently produces only its own posts without coordination overhead.
fn place_lamp_refs(
    roads: &[(u8, Vec<(i32, i32)>)],
    road_half_width: i32,
    world_scale: f64,
    cell_col: i32,
    cell_row: i32,
    cell_x: i32,
    cell_y: i32,
    ground: &Ground,
    global_min: i32,
    effective_scale: i32,
    water_height_game: Option<f32>,
    next_fid: &mut u32,
    occ: &mut OccupancyGrid,
) -> (Vec<u8>, u32) {
    let mut refs = Vec::new();
    let mut count = 0u32;

    if roads.is_empty() {
        return (refs, count);
    }

    // Scale-dependent keep probability.  At small scales (e.g. 0.142×) the
    // world is compressed so fewer lamp posts are appropriate; at scales ≥ 1
    // the count is capped at the scale-1 baseline.
    //   p(s) = clamp(0.5 × s^1.53, 0, 0.5)
    //   s = 0.142 → p ≈ 0.025  (~97.5 % fewer than scale 1)
    //   s = 1.00  → p = 0.50   (baseline — half of candidate posts kept)
    //   s ≥ 1.00  → p = 0.50   (capped; no extra posts at larger scales)
    let keep_prob = (0.5_f64 * world_scale.powf(1.53)).clamp(0.0, 0.5);

    let cell_min_ax = cell_col * BLOCKS_PER_CELL;
    let cell_max_ax = cell_min_ax + BLOCKS_PER_CELL;
    let cell_min_az = cell_row * BLOCKS_PER_CELL;
    let cell_max_az = cell_min_az + BLOCKS_PER_CELL;

    // Lamp sits just outside the road edge.
    let offset_dist = (road_half_width + 2) as f64;

    let mut emit = |px: i32, pz: i32| {
        if px < cell_min_ax || px >= cell_max_ax || pz < cell_min_az || pz >= cell_max_az {
            return;
        }

        // Convert arnis (x, z) to FNV local row (0 = south, increases northward).
        let local_col     = px - cell_min_ax;
        let local_row_fnv = BLOCKS_PER_CELL - 1 - (pz - cell_min_az);

        let game_x = cell_x as f32 * CELL_GAME_UNITS + local_col as f32 * 128.0 + 64.0;
        let game_y = cell_y as f32 * CELL_GAME_UNITS + local_row_fnv as f32 * 128.0 + 64.0;

        let raw_h  = ground.level(XZPoint::new(px, pz));
        let game_z = ((raw_h - global_min) * effective_scale + HEIGHT_MARGIN) as f32 * 8.0;

        if water_height_game.map_or(true, |wl| game_z >= wl) {
            refs.extend(build_tree_refr(*next_fid, LAMP_POST_FID, game_x, game_y, game_z, 0.0));
            *next_fid += 1;
            count += 1;
        }
    };

    for (_, polyline) in roads {
        for segment in polyline.windows(2) {
            let (x0, z0) = (segment[0].0 as f64, segment[0].1 as f64);
            let (x1, z1) = (segment[1].0 as f64, segment[1].1 as f64);
            let dx = x1 - x0;
            let dz = z1 - z0;
            let seg_len = (dx * dx + dz * dz).sqrt();
            if seg_len < 1.0 {
                continue;
            }

            // Perpendicular unit vector (left side of travel direction in arnis XZ).
            // CW 90°: (dx, dz) → (dz, -dx); scale to offset_dist.
            let perp_x = dz / seg_len * offset_dist;
            let perp_z = -dx / seg_len * offset_dist;

            // Number of evenly-spaced lamp posts along this segment.
            let num_steps = ((seg_len / LAMP_SPACING_BLOCKS as f64).floor() as i32).max(1);

            for step in 0..=num_steps {
                let t = (step as f64 * LAMP_SPACING_BLOCKS as f64).min(seg_len) / seg_len;
                let cx = x0 + dx * t;
                let cz = z0 + dz * t;

                // Scale-dependent thinning — skip this pair of posts based on
                // a deterministic RNG seeded from the centreline position.
                let mut rng = coord_rng(cx.round() as i32, cz.round() as i32, 0x4C414D50_00000000u64);
                if !rng.random_bool(keep_prob) {
                    continue;
                }

                // Place on both sides of the road, skipping occupied positions.
                let px_l = (cx + perp_x).round() as i32;
                let pz_l = (cz + perp_z).round() as i32;
                let px_r = (cx - perp_x).round() as i32;
                let pz_r = (cz - perp_z).round() as i32;

                if occ.is_clear(px_l, pz_l, OCC_RADIUS_LAMP) {
                    emit(px_l, pz_l);
                    occ.mark(px_l, pz_l, OCC_RADIUS_LAMP);
                }
                if occ.is_clear(px_r, pz_r, OCC_RADIUS_LAMP) {
                    emit(px_r, pz_r);
                    occ.mark(px_r, pz_r, OCC_RADIUS_LAMP);
                }
            }
        }
    }

    (refs, count)
}

// --- public entry point ---

pub fn generate_fnv_esm(
    ground: &Ground,
    bbox: &LLBBox,
    xzbbox: &XZBBox,
    output_dir: &Path,
    water_level: Option<f32>,
    world_scale: f64,
    roads: &[(u8, Vec<(i32, i32)>)],
    terrain_only: bool,
) -> Result<(), String> {
    let max_x = xzbbox.max_x();
    let max_z = xzbbox.max_z();

    let num_cols = ((max_x + BLOCKS_PER_CELL - 1) / BLOCKS_PER_CELL) as usize;
    let num_rows = ((max_z + BLOCKS_PER_CELL - 1) / BLOCKS_PER_CELL) as usize;

    if num_cols == 0 || num_rows == 0 {
        return Err("Bounding box too small to generate any FNV cells".to_string());
    }

    println!(
        "  Generating FNV worldspace: {}×{} cells ({} total)...",
        num_cols,
        num_rows,
        num_cols * num_rows
    );

    // Single pass: find the global minimum height and the largest gradient
    // between any two horizontally-adjacent vertices in the whole world.
    //
    // The gradient bound lets us pick the largest HEIGHT_SCALE that guarantees
    // no VHGT delta overflows i8 (-128..127).  Without this, a steep cliff
    // causes clamping which corrupts the reconstructed border height, producing
    // a seam even though both cells sample the same arnis coordinate.
    let total_verts_x = num_cols * (VERTS - 1) + 1; // one shared vertex per border
    let total_verts_z = num_rows * (VERTS - 1) + 1;

    let mut global_min = i32::MAX;
    let mut max_grad = 0i32;
    // Accumulate terrain heights at LC_WATER vertices for auto water-level detection.
    let mut water_height_sum: i64 = 0;
    let mut water_height_count: usize = 0;

    for zi in 0..total_verts_z {
        for xi in 0..total_verts_x {
            let pt = XZPoint::new(xi as i32, zi as i32);
            let h = ground.level(pt);
            if h < global_min {
                global_min = h;
            }
            if xi > 0 {
                let h_prev = ground.level(XZPoint::new(xi as i32 - 1, zi as i32));
                max_grad = max_grad.max((h - h_prev).abs());
            }
            if zi > 0 {
                let h_prev = ground.level(XZPoint::new(xi as i32, zi as i32 - 1));
                max_grad = max_grad.max((h - h_prev).abs());
            }
            // Collect heights at water-classified vertices.
            if ground.cover_class(pt) == land_cover::LC_WATER {
                water_height_sum += h as i64;
                water_height_count += 1;
            }
        }
    }

    // Effective scale: boost HEIGHT_SCALE inversely with world scale so terrain
    // doesn't appear flat at sub-1× scales (e.g. 1/7 scale for FNV).
    // boost = (1 / world_scale)^0.5 — at scale=1.0 no change; at scale=0.142 ≈ 2.65×.
    // Then cap so max_grad * scale ≤ 127, guaranteeing no VHGT delta overflow.
    let scale_boost = (1.0_f64 / world_scale.max(0.01)).powf(0.5).max(1.0);
    let base_scale = (HEIGHT_SCALE as f64 * scale_boost).round() as i32;
    let effective_scale = if max_grad > 0 {
        base_scale.min(127 / max_grad)
    } else {
        base_scale
    }
    .max(1); // always at least 1× so flat terrain still has a VHGT value

    // Resolve effective water level (priority order):
    //   1. Explicit --fnv-water-level override.
    //   2. Average terrain height of LC_WATER-classified vertices, converted to
    //      FNV game units using the same scale as sample_heights.
    //      1 VHGT unit = 8 FNV game units.
    //   3. None (no water placed) if no water pixels were found.
    let effective_water_level: Option<f32> = water_level.or_else(|| {
        if water_height_count == 0 {
            return None;
        }
        let avg_water_mc_y = (water_height_sum / water_height_count as i64) as i32;
        let vhgt_units = (avg_water_mc_y - global_min) * effective_scale + HEIGHT_MARGIN;
        // Terrain heights are always exact multiples of 8 game units.  Adding 2
        // places the water plane just above the surface, preventing Z-fighting
        // without visibly elevating the waterline.
        Some(vhgt_units as f32 * 8.0 + 2.0)
    });

    println!(
        "  Terrain: min_height={}, max_gradient={}, height_scale={}, water={}",
        global_min,
        max_grad,
        effective_scale,
        effective_water_level
            .map(|h| format!("{:.0} game-units{}", h, if water_level.is_none() { " (auto)" } else { "" }))
            .unwrap_or_else(|| "none".to_string())
    );

    // Default texture for vertices whose land-cover class is 0 (data unavailable).
    // ESA WorldCover only covers −60° to +84° latitude; outside that range we
    // pick TEXTURE_SNOW for polar regions rather than falling back to dirt.
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let default_texture: u32 = if !ground.has_land_cover() && center_lat.abs() > 60.0 {
        println!("  Land cover unavailable (outside ESA WorldCover range); defaulting to snow texture");
        TEXTURE_SNOW
    } else {
        TEXTURE_DIRT
    };

    // Build road raster — paint asphalt texture onto road-covered terrain blocks.
    // Skip if the world scale is too small for roads to span even a single block.
    let road_half_width = road_half_width_blocks(world_scale);
    let road_grid: Option<RoadGrid> = if !roads.is_empty() /*&& road_half_width >= 1*/ {
        println!(
            "  Building road grid ({} road segments, half-width {} blocks)...",
            roads.len(),
            road_half_width
        );
        let road_grid_w = num_cols as i32 * BLOCKS_PER_CELL;
        let road_grid_h = num_rows as i32 * BLOCKS_PER_CELL;
        Some(build_road_grid(roads, road_grid_w, road_grid_h, road_half_width))
    } else {
        if !roads.is_empty() {
            println!(
                "  Skipping road painting (world scale {:.3} too small — roads < 1 block wide)",
                world_scale
            );
        }
        None
    };
    println!("  Done building road grid");

    // Allocate FormIDs from testesm's nextObjectID upward so they share the
    // same verified namespace as the WRLD FormID (0x01000ADD).
    let mut next_fid: u32 = PLUGIN_IDX | TESTESM_NEXT_OID_BASE;

    // Center the worldspace on cell (0,0) — FNV convention.
    // x_offset shifts column indices so the westernmost column starts at a
    // negative X; y_offset does the same for rows / northward Y.
    let x_offset = num_cols as i32 / 2;
    let y_offset = num_rows as i32 / 2;

    struct CellInfo {
        cell_x: i32,
        cell_y: i32,
        cell_fid: u32,
        land_fid: u32,
        vhgt_offset: f32,
        deltas: Vec<i8>,
        vnml: Vec<u8>,
        water_height_game: Option<f32>,
        quads: [QuadTexture; 4],
        tree_refs: Vec<u8>,
        tree_count: u32,
        rock_refs: Vec<u8>,
        rock_count: u32,
        shrub_refs: Vec<u8>,
        shrub_count: u32,
        lamp_refs: Vec<u8>,
        lamp_count: u32,
        house_refs: Vec<u8>,
        house_count: u32,
        commercial_refs: Vec<u8>,
        commercial_count: u32,
    }

    let mut cells: Vec<CellInfo> = Vec::with_capacity(num_cols * num_rows);
    if !terrain_only {
        print!("  Placing objects");
        for row in 0..num_rows {
            for col in 0..num_cols {
                print!(".");
                io::stdout().flush().unwrap();
                let cell_x = col as i32 - x_offset;
                // Arnis Z increases southward; FNV Y increases northward.
                let cell_y = (num_rows - 1 - row) as i32 - y_offset;
                let cell_fid = next_fid;
                let land_fid = next_fid + 1;
                next_fid += 2;

                let heights =
                    sample_heights(ground, col as i32, row as i32, global_min, effective_scale);
                let (vhgt_offset, deltas) = encode_vhgt(&heights);
                let vnml = compute_vnml(&heights);
                let quads = compute_quad_textures(ground, col, row, &heights, default_texture, road_grid.as_ref());

                // Shared occupancy grid for this cell.  Buildings are placed first so
                // they take priority; vegetation fills in the remaining space.
                let mut occ = OccupancyGrid::new(col as i32, row as i32);

                let (house_refs, house_count) = place_house_refs(
                    roads,
                    road_grid.as_ref(),
                    road_half_width,
                    world_scale,
                    col as i32,
                    row as i32,
                    cell_x,
                    cell_y,
                    ground,
                    global_min,
                    effective_scale,
                    effective_water_level,
                    &mut next_fid,
                    &mut occ,
                );

                let (commercial_refs, commercial_count) = place_commercial_refs(
                    roads,
                    road_grid.as_ref(),
                    road_half_width,
                    world_scale,
                    col as i32,
                    row as i32,
                    cell_x,
                    cell_y,
                    ground,
                    global_min,
                    effective_scale,
                    effective_water_level,
                    &mut next_fid,
                    &mut occ,
                );

                let (lamp_refs, lamp_count) = place_lamp_refs(
                    roads,
                    road_half_width,
                    world_scale,
                    col as i32,
                    row as i32,
                    cell_x,
                    cell_y,
                    ground,
                    global_min,
                    effective_scale,
                    effective_water_level,
                    &mut next_fid,
                    &mut occ,
                );

                let (tree_refs, tree_count) = place_tree_refs(
                    ground,
                    col as i32,
                    row as i32,
                    cell_x,
                    cell_y,
                    global_min,
                    effective_scale,
                    effective_water_level,
                    &mut next_fid,
                    &mut occ,
                );

                let (rock_refs, rock_count) = place_rock_refs(
                    ground,
                    col as i32,
                    row as i32,
                    cell_x,
                    cell_y,
                    global_min,
                    effective_scale,
                    effective_water_level,
                    &mut next_fid,
                    &mut occ,
                );

                let (shrub_refs, shrub_count) = place_shrub_refs(
                    ground,
                    col as i32,
                    row as i32,
                    cell_x,
                    cell_y,
                    global_min,
                    effective_scale,
                    effective_water_level,
                    &mut next_fid,
                    &mut occ,
                );

                cells.push(CellInfo {
                    cell_x,
                    cell_y,
                    cell_fid,
                    land_fid,
                    vhgt_offset,
                    deltas,
                    vnml,
                    water_height_game: effective_water_level,
                    quads,
                    tree_refs,
                    tree_count,
                    rock_refs,
                    rock_count,
                    shrub_refs,
                    shrub_count,
                    lamp_refs,
                    lamp_count,
                    house_refs,
                    house_count,
                    commercial_refs,
                    commercial_count,
                });
            }
        }
        println!("Done!");
    } else {
        println!("Skipping object placement");
    }

    // Group cells into exterior blocks (div_euclid 8) and subblocks (div_euclid 2).

    let mut blocks: BTreeMap<(i32, i32), BTreeMap<(i32, i32), Vec<usize>>> = BTreeMap::new();
    for (idx, cell) in cells.iter().enumerate() {
        let bx = cell.cell_x.div_euclid(8);
        let by = cell.cell_y.div_euclid(8);
        let sx = cell.cell_x.div_euclid(2);
        let sy = cell.cell_y.div_euclid(2);
        blocks
            .entry((bx, by))
            .or_default()
            .entry((sx, sy))
            .or_default()
            .push(idx);
    }

    // Build world-children GRUP content.
    let mut world_children_content = Vec::new();

    for ((bx, by), subblocks) in &blocks {
        let mut block_content = Vec::new();

        for ((sx, sy), cell_indices) in subblocks {
            let mut subblock_content = Vec::new();

            for &idx in cell_indices {
                let cell = &cells[idx];

                let cell_rec = build_cell_record(
                    cell.cell_fid,
                    cell.cell_x,
                    cell.cell_y,
                    cell.water_height_game,
                );
                let land_rec = build_land_record(
                    cell.land_fid,
                    cell.vhgt_offset,
                    &cell.deltas,
                    &cell.vnml,
                    &cell.quads,
                );

                let mut cell_label = [0u8; 4];
                cell_label.copy_from_slice(&cell.cell_fid.to_le_bytes());

                // Type 9 = cell temp children: LAND record + all object REFRs.
                let mut temp_children_content = Vec::new();
                temp_children_content.extend_from_slice(&land_rec);
                temp_children_content.extend_from_slice(&cell.tree_refs);
                temp_children_content.extend_from_slice(&cell.rock_refs);
                temp_children_content.extend_from_slice(&cell.shrub_refs);
                temp_children_content.extend_from_slice(&cell.lamp_refs);
                temp_children_content.extend_from_slice(&cell.house_refs);
                temp_children_content.extend_from_slice(&cell.commercial_refs);

                let mut tmp_grup = Vec::new();
                push_grup(&mut tmp_grup, cell_label, 9, &temp_children_content);

                let mut cell_children_grup = Vec::new();
                push_grup(&mut cell_children_grup, cell_label, 6, &tmp_grup); // type 6 = cell children

                subblock_content.extend_from_slice(&cell_rec);
                subblock_content.extend_from_slice(&cell_children_grup);
            }

            // FNV exterior subblock GRUP label: Y-coordinate first, then X (LE i16 pairs).
            let sub_label = xy_label(*sy as i16, *sx as i16);
            push_grup(&mut block_content, sub_label, 5, &subblock_content);
        }

        // FNV exterior block GRUP label: Y-coordinate first, then X (LE i16 pairs).
        let blk_label = xy_label(*by as i16, *bx as i16);
        push_grup(&mut world_children_content, blk_label, 4, &block_content);
    }

    // WRLD record (uses testesm's FormID).
    let min_cell_x = -x_offset;
    let max_cell_x = num_cols as i32 - 1 - x_offset;
    let min_cell_y = -y_offset;
    let max_cell_y = num_rows as i32 - 1 - y_offset;
    let wrld_rec = build_wrld_record(min_cell_x, max_cell_x, min_cell_y, max_cell_y, effective_water_level);

    // World-children GRUP (type 1, label = WRLD FormID bytes).
    let mut wc_label = [0u8; 4];
    wc_label.copy_from_slice(&TESTESM_WRLD_FID.to_le_bytes());
    let mut world_children_grup = Vec::new();
    push_grup(&mut world_children_grup, wc_label, 1, &world_children_content);

    // Top-level WRLD GRUP (type 0, label = b"WRLD").
    let mut wrld_grup_content = Vec::new();
    wrld_grup_content.extend_from_slice(&wrld_rec);
    wrld_grup_content.extend_from_slice(&world_children_grup);
    let mut wrld_grup = Vec::new();
    push_grup(&mut wrld_grup, *b"WRLD", 0, &wrld_grup_content);

    // Clone testesm's TES4 record verbatim, then patch the two fields that
    // reflect our content size.
    let mut tes4 = TESTESM_BYTES[..TESTESM_TES4_LEN].to_vec();

    let total_refrs: u32 = cells.iter().map(|c| c.tree_count + c.rock_count + c.shrub_count + c.lamp_count + c.house_count + c.commercial_count).sum();
    let num_records = 1u32 + cells.len() as u32 * 2 + total_refrs; // WRLD + CELL + LAND + REFRs
    tes4[TES4_NUM_RECORDS_OFFSET..TES4_NUM_RECORDS_OFFSET + 4]
        .copy_from_slice(&num_records.to_le_bytes());

    // nextObjectID: store lower 24 bits of next available FormID.
    let next_oid_local = next_fid & 0x00FF_FFFF;
    tes4[TES4_NEXT_OID_OFFSET..TES4_NEXT_OID_OFFSET + 4]
        .copy_from_slice(&next_oid_local.to_le_bytes());

    let mut file_bytes = Vec::new();
    file_bytes.extend_from_slice(&tes4);
    file_bytes.extend_from_slice(&wrld_grup);

    let out_path = output_dir.join("arnis_worldspace.esm");
    fs::write(&out_path, &file_bytes)
        .map_err(|e| format!("Failed to write ESM file: {}", e))?;

    println!(
        "{} FNV worldspace ESM saved to: {}",
        "Done!".green().bold(),
        out_path.display()
    );

    Ok(())
}
