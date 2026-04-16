use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::deterministic_rng::coord_rng;
use crate::ground::Ground;
use crate::land_cover;
use colored::Colorize;
use rand::Rng;
use std::collections::BTreeMap;
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

/// Juniper tree placed in tree-cover terrain cells.
const JUNIPER_FID: u32 = 0x001479E2;

/// Grid spacing (in arnis blocks) between candidate tree placement points.
/// A value of 8 gives a 4×4 = 16-point grid per cell; with the placement
/// probability this yields a natural-looking low-to-medium density.
const TREE_GRID_SPACING: i32 = 8;

// Rock variants — sizes are approximate model extents.
const ROCK_SMALL_CLUSTER_FID: u32 = 0x00119885; // ~2 m diameter cluster
const ROCK_TALL_FID: u32 = 0x00119882;           // ~2 m wide × 4 m tall
const ROCK_MEDIUM_FID: u32 = 0x00119887;         // ~4 m × 8 m
const ROCK_LARGE_FID: u32 = 0x00119888;          // ~8 m × 16 m

/// Grid spacing for rock placement.  Coarser than trees to avoid wall-of-rocks
/// effect and to leave room for the larger variants.
const ROCK_GRID_SPACING: i32 = 10;

/// Desert shrub/bush placed in shrubland and grassland cells.
const SHRUB_FID: u32 = 0x000EC8D2;

/// Grid spacing for shrub placement.  Denser than rocks; shrubs are small.
const SHRUB_GRID_SPACING: i32 = 6;

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
    polylines: &[Vec<(i32, i32)>],
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

    for polyline in polylines {
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
        .flat_map(|(i, pl)| {
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

/// Generate REFR records for vegetation in one cell.
///
/// Iterates a regular grid across the cell at `TREE_GRID_SPACING`-block
/// intervals. At each grid point, samples the land-cover class and places a
/// Juniper tree if the class is LC_TREE_COVER. A deterministic per-coordinate
/// RNG decides whether to actually place (density thinning) and adds small
/// positional/rotational jitter.
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
            if lc == land_cover::LC_TREE_COVER {
                let mut rng = coord_rng(arnis_x, arnis_z, 0x4A554E49_50455200);

                // ~60% placement probability to avoid uniform grid appearance.
                if rng.random_bool(0.6) {
                    // Jitter within ±48 game units (< half a block = 64 units).
                    let jitter_x: f32 = rng.random_range(-48.0_f32..48.0_f32);
                    let jitter_y: f32 = rng.random_range(-48.0_f32..48.0_f32);
                    // Random Z rotation 0..2π.
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

                    // Skip trees submerged below the water surface.
                    if water_height_game.map_or(true, |wl| game_z >= wl) {
                        refs.extend(build_tree_refr(*next_fid, JUNIPER_FID, game_x, game_y, game_z, rot_z));
                        *next_fid += 1;
                        count += 1;
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

                    if water_height_game.map_or(true, |wl| game_z >= wl) {
                        refs.extend(build_tree_refr(*next_fid, rock_fid, game_x, game_y, game_z, rot_z));
                        *next_fid += 1;
                        count += 1;
                    }
                }
            }

            local_col += ROCK_GRID_SPACING;
        }
        local_row_fnv += ROCK_GRID_SPACING;
    }

    (refs, count)
}

/// Generate REFR records for shrubs/bushes in one cell.
///
/// Dense in `LC_SHRUBLAND`, sparser in `LC_GRASSLAND` and `LC_BARE` (scrubby
/// outskirts of rocky areas).  Rotation is fully random — the asset looks fine
/// from any angle.
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
                land_cover::LC_BARE      => 0.15,
                _ => 0.0,
            };

            if place_prob > 0.0 {
                let mut rng = coord_rng(arnis_x, arnis_z, 0x42555348_00000000);

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

                    if water_height_game.map_or(true, |wl| game_z >= wl) {
                        refs.extend(build_tree_refr(*next_fid, SHRUB_FID, game_x, game_y, game_z, rot_z));
                        *next_fid += 1;
                        count += 1;
                    }
                }
            }

            local_col += SHRUB_GRID_SPACING;
        }
        local_row_fnv += SHRUB_GRID_SPACING;
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
    roads: &[Vec<(i32, i32)>],
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
    }

    let mut cells: Vec<CellInfo> = Vec::with_capacity(num_cols * num_rows);

    for row in 0..num_rows {
        for col in 0..num_cols {
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
            });
        }
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

    let total_refrs: u32 = cells.iter().map(|c| c.tree_count + c.rock_count + c.shrub_count).sum();
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
