use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::LLBBox;
use crate::ground::Ground;
use crate::land_cover;
use colored::Colorize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const BLOCKS_PER_CELL: i32 = 32;
const VERTS: usize = 33;
const HEIGHT_MARGIN: i32 = 16;
const FNV_FORM_VERSION: u16 = 15;
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

/// Sample a 33×33 vertex grid for one cell and encode it as VHGT deltas.
///
/// FNV VHGT convention:
///   row 0 = SOUTH edge of the cell (higher arnis Z, since Z increases south)
///   row 32 = NORTH edge of the cell (lower arnis Z)
///   col 0 = WEST edge, col 32 = EAST edge
///
/// Flipping the row direction ensures that adjacent cells share the same
/// terrain sample at their border: SOUTH of cell (cell_y) uses the same
/// arnis z-coordinate as NORTH of cell (cell_y - 1).
/// Sample and scale the 33×33 vertex grid for one cell.
/// Returns the height grid in VHGT units (offset-zeroed, scaled).
fn sample_heights(
    ground: &Ground,
    cell_col: i32,
    cell_row: i32,
    global_min: i32,
    scale: i32,
) -> [[i32; VERTS]; VERTS] {
    let mut heights = [[0i32; VERTS]; VERTS];
    for row in 0..VERTS {
        for col in 0..VERTS {
            let x = cell_col * BLOCKS_PER_CELL + col as i32;
            // row 0 = south (large arnis z), row 32 = north (small arnis z)
            let z = cell_row * BLOCKS_PER_CELL + (VERTS - 1 - row) as i32;
            let raw_h = ground.level(XZPoint::new(x, z));
            heights[row][col] = (raw_h - global_min) * scale + HEIGHT_MARGIN;
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

/// Sample the 17×17 vertex grid of each quadrant and build smoothly-blended
/// per-vertex texture assignments.
///
/// For every vertex, a `BLEND_RADIUS`-block neighbourhood is sampled in arnis
/// space. Because the sampling is in world coordinates rather than
/// quadrant-relative coordinates, it naturally crosses quadrant and cell
/// boundaries, eliminating hard seams at those edges.
///
/// Each texture's share of the neighbourhood becomes its VTXT opacity, so
/// transition zones blend smoothly instead of snapping to a single texture.
///
/// Quadrant layout in the 33×33 LAND vertex grid (row 0 = south):
///   NW (0): rows 16–32, cols  0–16
///   NE (1): rows 16–32, cols 16–32
///   SW (2): rows  0–16, cols  0–16
///   SE (3): rows  0–16, cols 16–32
/// `default_texture` is used for any vertex whose land-cover class has no
/// explicit mapping (class 0 = data unavailable).  Pass `TEXTURE_SNOW` for
/// polar regions where ESA WorldCover has no coverage.
fn compute_quad_textures(
    ground: &Ground,
    cell_col: usize,
    cell_row: usize,
    default_texture: u32,
) -> [QuadTexture; 4] {
    // (land_row_start, land_col_start) for quadrants NW=0, NE=1, SW=2, SE=3
    const QUAD_ORIGIN: [(usize, usize); 4] = [(16, 0), (16, 16), (0, 0), (0, 16)];

    // Blend radius in arnis blocks.  Radius 8 → 17×17 = 289-sample window.
    // This covers 8 vertices (= ¼ of a cell) on each side of any boundary,
    // giving a 16-vertex gradient across cell and quadrant edges.
    const BLEND_RADIUS: i32 = 8;
    const BLEND_DIAM: usize = (BLEND_RADIUS * 2 + 1) as usize;
    const BLEND_TOTAL: f32 = (BLEND_DIAM * BLEND_DIAM) as f32;
    // Include any vertex with at least one minority sample (1/289 ≈ 0.003).
    // Raising this threshold was the primary cause of lingering hard seams in
    // the previous implementation.
    const MIN_OPACITY: f32 = 1.0 / BLEND_TOTAL;

    // ── Pre-sample texture IDs ──────────────────────────────────────────────
    // Build a flat grid covering the cell area plus a BLEND_RADIUS-wide border
    // so the inner blending loop just indexes an array instead of calling
    // cover_class for every (vertex × neighbour) pair.
    //
    // Grid is indexed [sz * sample_size + sx] where:
    //   sx = arnis_x - origin_ax,  sz = arnis_z - origin_az
    let sample_size = BLOCKS_PER_CELL as usize + BLEND_RADIUS as usize * 2 + 1; // 49
    let origin_ax = cell_col as i32 * BLOCKS_PER_CELL - BLEND_RADIUS;
    let origin_az = cell_row as i32 * BLOCKS_PER_CELL - BLEND_RADIUS;

    let mut tex_grid: Vec<u32> = vec![default_texture; sample_size * sample_size];
    for sz in 0..sample_size {
        for sx in 0..sample_size {
            let ax = origin_ax + sx as i32;
            let az = origin_az + sz as i32;
            let lc = ground.cover_class(XZPoint::new(ax, az));
            tex_grid[sz * sample_size + sx] = if lc == 0 {
                default_texture
            } else {
                texture_for_cover(lc)
            };
        }
    }

    // ── Per-quadrant texture assignment ────────────────────────────────────
    let quads: Vec<QuadTexture> = (0..4)
        .map(|quad| {
            let (row_start, col_start) = QUAD_ORIGIN[quad];

            let mut vertex_fracs: Vec<BTreeMap<u32, f32>> = Vec::with_capacity(17 * 17);
            let mut quad_weights: BTreeMap<u32, f32> = BTreeMap::new();

            for qr in 0..17usize {
                for qc in 0..17usize {
                    let land_row = row_start + qr;
                    let land_col = col_start + qc;
                    // World coordinates for this vertex (row-flip matches sample_heights).
                    let ax = cell_col as i32 * BLOCKS_PER_CELL + land_col as i32;
                    let az = cell_row as i32 * BLOCKS_PER_CELL + (VERTS - 1 - land_row) as i32;

                    // Centre of this vertex in the pre-sampled grid.
                    let cx = (ax - origin_ax) as usize;
                    let cz = (az - origin_az) as usize;

                    // Count each texture in the BLEND_RADIUS window using the grid.
                    let mut counts: BTreeMap<u32, i32> = BTreeMap::new();
                    for dz in 0..BLEND_DIAM {
                        let sz = cz + dz - BLEND_RADIUS as usize; // always in-bounds
                        for dx in 0..BLEND_DIAM {
                            let sx = cx + dx - BLEND_RADIUS as usize;
                            let fid = tex_grid[sz * sample_size + sx];
                            *counts.entry(fid).or_insert(0) += 1;
                        }
                    }

                    let mut fracs: BTreeMap<u32, f32> = BTreeMap::new();
                    for (&fid, &cnt) in &counts {
                        let f = cnt as f32 / BLEND_TOTAL;
                        fracs.insert(fid, f);
                        *quad_weights.entry(fid).or_insert(0.0) += f;
                    }
                    vertex_fracs.push(fracs);
                }
            }

            // Base = texture with the highest aggregate weight in this quadrant.
            let base_fid = quad_weights
                .iter()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(f, _)| *f)
                .unwrap_or(TEXTURE_DIRT);

            // ATXT layers for minority textures.  Every vertex with at least one
            // minority sample gets a VTXT entry — this is what produces the gradient.
            let mut extra: BTreeMap<u32, Vec<(u16, f32)>> = BTreeMap::new();
            for (i, fracs) in vertex_fracs.iter().enumerate() {
                for (&fid, &opacity) in fracs {
                    if fid != base_fid && opacity >= MIN_OPACITY {
                        extra.entry(fid).or_default().push((i as u16, opacity));
                    }
                }
            }

            QuadTexture {
                base_fid,
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

// --- public entry point ---

pub fn generate_fnv_esm(
    ground: &Ground,
    bbox: &LLBBox,
    xzbbox: &XZBBox,
    output_dir: &Path,
    water_level: Option<f32>,
    world_scale: f64,
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
        Some(vhgt_units as f32 * 8.0)
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
            let quads = compute_quad_textures(ground, col, row, default_texture);

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

                let mut tmp_grup = Vec::new();
                push_grup(&mut tmp_grup, cell_label, 9, &land_rec); // type 9 = cell temp children

                let mut cell_children_grup = Vec::new();
                push_grup(&mut cell_children_grup, cell_label, 6, &tmp_grup); // type 6 = cell children

                subblock_content.extend_from_slice(&cell_rec);
                subblock_content.extend_from_slice(&cell_children_grup);
            }

            let sub_label = xy_label(*sx as i16, *sy as i16);
            push_grup(&mut block_content, sub_label, 5, &subblock_content);
        }

        let blk_label = xy_label(*bx as i16, *by as i16);
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

    let num_records = 1u32 + cells.len() as u32 * 2; // WRLD + CELL + LAND per cell
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
