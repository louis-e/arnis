//! Facade textures as item display entities (Java 1.21.4+): the `photos`
//! facade mode.
//!
//! An item display is a free entity, not a hanging one, so it can sit anywhere
//! at any angle, nothing has to be solid behind it, nothing may block the air
//! in front of it, and it never drops. A painting entity, which is what an
//! earlier mode hung, is nailed to a block face: a wall that runs diagonally
//! through the block grid became a staircase of axis-aligned panels, and two
//! panels meeting at a step fought over the air cell between them, so one of
//! the two faces had to stay bare.
//!
//! So this mode puts **one flat quad on the wall's true line**, whatever angle
//! that line runs at. A wall is cut only when it is longer or taller than
//! `MAX_PANEL` blocks, and then only because one texture per panel is stitched
//! into the game's block atlas.
//!
//! A panel carries the photograph at its own metre scale and nothing else. On
//! a slope the shell's floor sits at the building's highest corner and the
//! wall is filled down to the terrain below that, so the downhill end of a
//! panel stops short of the street and the fill there stays bare. That gap was
//! once covered by repeating the crop's bottom row down to the ground, which
//! read as a vertical smear: the photograph ends at the ground the camera saw,
//! and there is nothing honest to put below it. The block facades anchor the
//! texture at the same first wall block and never reach below it either.
//!
//! Three steps:
//! * `collect` runs when a building's wall ring has been built and records one
//!   candidate per textured wall: its cells, its texture columns and how tall
//!   it was built. A building is processed by every tile it overlaps, so the
//!   first tile to reach a wall claims it.
//! * `finalize` runs once every block is final, before the world is saved
//!   (under stream-to-disk eviction `flush_region` does the same for each
//!   region right before it leaves memory, since an entity written into a
//!   flushed region is lost). It works out each candidate's quad, writes one
//!   `minecraft:item_display` per piece and crops that piece out of the 8 px/m
//!   wall texture.
//! * `write_packs` runs once the world is saved. An item's model is chosen by
//!   the `minecraft:item_model` component, which is pure resource pack, so
//!   there is no data pack: everything goes into `<world>/resources.zip` (and
//!   the same file under `resourcepacks/`, where 26.1 looks).
//!
//! Per panel the pack carries three files: the item model definition
//! (`assets/arnis/items/<name>.json`), the model itself
//! (`assets/arnis/models/item/<name>.json`) and the texture
//! (`assets/arnis/textures/block/<name>.png`). The texture goes under
//! `textures/block/` because vanilla's `blocks.json` atlas has a `directory`
//! source over `block/` that spans every namespace, so the panels are stitched
//! into the block atlas without this pack shipping an atlas file of its own.
//!
//! 1.21.11 split item textures out into an `items` atlas of their own, which
//! does not change this: a model's textures are looked up in the items atlas
//! first and then in the blocks one, and vanilla's own block items work the
//! same way (`items/dirt.json` draws `block/dirt`). What the atlas a model
//! landed in does decide is the render pass, see `ITEM`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{BufWriter, Cursor, Seek, Write};
use std::path::Path;
use std::sync::Mutex;

use fastnbt::Value;
use fnv::{FnvHashMap, FnvHashSet};
use image::{RgbImage, RgbaImage};
use rayon::prelude::*;

use super::atlas::{atlas_budget, MIN_PX_PER_BLOCK};
use super::facades::{self, FacadeStore};
use crate::colors::RGBTuple;
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};
use crate::world_editor::WorldEditor;

/// Longest panel edge in blocks. An item display has no size cap of its own,
/// but one texture per panel is stitched into the block atlas, so a very long
/// wall is still cut into pieces of a size the atlas can hold.
pub const MAX_PANEL: i32 = 32;

/// How far the quad sits in front of the wall's outermost block corner, in
/// blocks. Enough that it never z-fights, small enough that it still reads as
/// the wall's own surface.
pub const PUSH_OUT: f64 = 0.06;

/// `view_range` 1.0 is 64 blocks times the client's entity distance scaling.
/// A facade has to stay up as long as the building itself is drawn.
const VIEW_RANGE: f32 = 4.0;

/// Namespace of the models and their textures.
const NAMESPACE: &str = "arnis";

/// The item the display carries. Its own model is never consulted: the
/// `minecraft:item_model` component replaces the lookup outright. An id that
/// does not resolve falls back to the game's missing-item model, never to the
/// carrier's own, so a pack that failed to load shows magenta panels rather
/// than no panels at all.
///
/// It is a **block** item, and that is why it is stone rather than something
/// inert like paper. The client picks the render pass of an item model from
/// the carrier item, not from the model: once `BlockModelWrapper` sees that
/// every quad of the model comes from the block atlas it asks its block render
/// type getter, which answers with the opaque `entity_cutout` pass for a block
/// item whose block is not a translucent one, and with the alpha-blended,
/// depth-sorted `item_entity_translucent_cull` pass for every other item. A
/// photograph is opaque, so the cutout pass is the right one: no per-quad
/// sorting against the world's other translucent geometry.
///
/// The translucent pass is not an invisible one, so the carrier item decides
/// how a panel is blended and never whether it is drawn. It renders into
/// `ITEM_ENTITY_TARGET`, and that target falls back to the main render target
/// whenever the level renderer holds no item entity target of its own, which
/// is every graphics setting except Fabulous; Fabulous does hold one and its
/// transparency chain composites it. Every dropped item goes through it.
const ITEM: &str = "minecraft:stone";

/// Resource pack format of 1.21.4, the first version where an item's model is
/// chosen by the `minecraft:item_model` component and resolved through
/// `assets/<ns>/items/`. The mcmeta declares support open-ended upwards, so
/// every later version loads the pack as well.
const RESOURCEPACK_FORMAT: u32 = 46;

/// Pixels per metre of the lab's `_tex.png` wall textures.
const TEX_PX_PER_M: f64 = 8.0;

/// Panels whose crop carries fewer valid pixels than this stay plain blocks.
const MIN_VALID_FRACTION: f64 = 0.25;

/// The wall's outward unit normal in Arnis' frame (x east, z south): the
/// perpendicular of the node A to node B direction `dir` that points the way
/// the axis-snapped normal `(snx, snz)` does, kept at its true angle rather
/// than snapped to an axis. None for a wall with no direction at all.
pub fn outward_normal(dir: (i32, i32), snx: i32, snz: i32) -> Option<(f64, f64)> {
    let (dx, dz) = (f64::from(dir.0), f64::from(dir.1));
    let len = (dx * dx + dz * dz).sqrt();
    // Dot product of the perpendicular (dz, -dx) with the snapped normal.
    let side = dz * f64::from(snx) - dx * f64::from(snz);
    let (nx, nz) = if len > 1e-9 && side > 0.0 {
        (dz / len, -dx / len)
    } else if len > 1e-9 && side < 0.0 {
        (-dz / len, dx / len)
    } else {
        // No direction, or a snapped normal along it: the snap is all there is.
        let l = ((snx * snx + snz * snz) as f64).sqrt();
        if l < 1e-9 {
            return None;
        }
        (f64::from(snx) / l, f64::from(snz) / l)
    };
    Some((nx, nz))
}

/// The viewer's right, standing outside the wall and looking at it: the
/// model's own +x axis once the quad has been turned onto `n`.
pub fn right_of(n: (f64, f64)) -> (f64, f64) {
    (n.1, -n.0)
}

/// Minecraft yaw in degrees of a quad whose front points along the unit normal
/// `n`. Yaw 0 faces south (+z) and +90 west (-x), so the facing vector is
/// (-sin yaw, cos yaw) and the yaw is its inverse.
///
/// Nothing writes a yaw: the turn lives in `left_rotation` alone, and the
/// entity's own `Rotation` stays at zero so the two cannot compound. This is
/// here to pin the convention the quaternion encodes.
#[cfg(test)]
pub fn yaw_deg(n: (f64, f64)) -> f64 {
    (-n.0).atan2(n.1).to_degrees()
}

/// The `left_rotation` quaternion [x, y, z, w] that turns the model's own +z
/// axis onto the unit normal `n`.
///
/// Minecraft's frame is right-handed (east cross up is south), so a turn of
/// theta about +y takes (0, 0, 1) to (sin theta, cos theta); asking for
/// (nx, nz) gives theta = atan2(nx, nz), which is minus the yaw.
///
/// Which of the model's two wide faces ends up looking at the street is not
/// the +z one: the item display renderer turns the model half a circle about
/// +y before it draws it, so that the item sits the way it does in an item
/// frame, and that turn happens inside this rotation. The face on the street
/// is therefore the model's `north` face, and `model_json` textures both wide
/// faces so it does not matter. See the note there for why the crop still
/// reads the right way round.
pub fn left_rotation(n: (f64, f64)) -> [f32; 4] {
    let half = n.0.atan2(n.1) / 2.0;
    [0.0, half.sin() as f32, 0.0, half.cos() as f32]
}

/// The world direction the model's +z face points once `q` (a turn about +y)
/// has been applied: the quaternion rotation of (0, 0, 1). The inverse of
/// `left_rotation`, so a test can read a quaternion back as a direction.
#[cfg(test)]
pub fn facing_of(q: [f32; 4]) -> (f64, f64) {
    let (y, w) = (f64::from(q[1]), f64::from(q[3]));
    (2.0 * y * w, w * w - y * y)
}

/// True when the wall texture has to be mirrored: its columns run from node A
/// to node B, and the panel's left edge must be the outside viewer's left.
pub fn flip_crop(dir: (i32, i32), n: (f64, f64)) -> bool {
    let (rx, rz) = right_of(n);
    f64::from(dir.0) * rx + f64::from(dir.1) * rz < 0.0
}

/// How far one wall cell carries the wall along its own line, in blocks. The
/// wall builder walks a Bresenham line, which advances one block along the
/// wall's major axis per cell, so a cell is one block of a straight wall and
/// about 1.41 of a wall at 45 degrees.
pub fn cell_step(dir: (i32, i32)) -> f64 {
    let major = dir.0.abs().max(dir.1.abs());
    if major == 0 {
        return 1.0;
    }
    let (dx, dz) = (f64::from(dir.0), f64::from(dir.1));
    (dx * dx + dz * dz).sqrt() / f64::from(major)
}

/// Where one panel hangs: the centre of its quad in world coordinates, its
/// size in blocks and the turn that puts its front onto the wall's normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub cx: f64,
    pub cy: f64,
    pub cz: f64,
    /// Width along the wall and height upwards, both in blocks.
    pub w: f64,
    pub h: f64,
    /// `transformation.left_rotation`, as [x, y, z, w].
    pub rot: [f32; 4],
}

/// The flat quad covering `cells` (wall cells of one wall, in any order)
/// between absolute world y `base_y` and `base_y + h`.
///
/// Along the wall the quad reaches half a cell step past the outer cell
/// centres. Reaching the end cells' outermost corners would want half of
/// `|rx| + |rz|` instead, which is the same number for a straight wall and for
/// one at 45 degrees and up to 0.07 blocks more for one in between, so an
/// oblique wall can leave a texel of its two end corners uncovered. Half a
/// step is used anyway because a step is what one cell of a uniform run
/// carries, so the pieces a wall is cut into at `MAX_PANEL` meet edge to edge
/// instead of overlapping and z-fighting along their seam.
///
/// Across the wall the quad is pushed clear of the outermost block corner of
/// every cell: a cell centre's own corner sticks out `0.5 * (|nx| + |nz|)`
/// along the normal, which is half a block for a straight wall and 0.71 for
/// one at 45 degrees, and a staircase would otherwise poke through its own
/// photograph.
pub fn quad_for(cells: &[(i32, i32)], n: (f64, f64), step: f64, base_y: i32, h: i32) -> Quad {
    let (rx, rz) = right_of(n);
    let (mut s_lo, mut s_hi) = (f64::MAX, f64::MIN);
    let mut t_max = f64::MIN;
    for &(bx, bz) in cells {
        let (px, pz) = (f64::from(bx) + 0.5, f64::from(bz) + 0.5);
        let s = px * rx + pz * rz;
        s_lo = s_lo.min(s);
        s_hi = s_hi.max(s);
        t_max = t_max.max(px * n.0 + pz * n.1);
    }
    let s_c = (s_lo + s_hi) / 2.0;
    let plane = t_max + 0.5 * (n.0.abs() + n.1.abs()) + PUSH_OUT;
    Quad {
        // (right, n) is an orthonormal basis of the ground plane, so a point
        // is just its two coordinates read back out along them.
        cx: s_c * rx + plane * n.0,
        cy: f64::from(base_y) + f64::from(h) / 2.0,
        cz: s_c * rz + plane * n.1,
        w: (s_hi - s_lo) + step,
        h: f64::from(h),
        rot: left_rotation(n),
    }
}

/// One panel's texture, resampled when the pack is written.
pub struct Panel {
    pub name: String,
    /// Panel size in blocks; not whole numbers on a wall that runs at an angle.
    pub w: f64,
    pub h: f64,
    pub tex: RgbImage,
}

/// The cells of one wall while a building is being collected, each with its
/// texture column, plus the sum of their axis-snapped outward normals, which
/// picks the side of the wall the building is not on.
#[derive(Default)]
struct WallCells {
    cells: Vec<(i32, i32, u16)>,
    snapped: (i32, i32),
}

/// One textured wall waiting for the finished world.
struct Candidate {
    way_id: u64,
    wall: u32,
    /// Sum of the axis-snapped normals of the wall's cells, which picks the
    /// side of the wall the building is not on.
    snapped: (i32, i32),
    /// Absolute y of wall row 0.
    base_y: i32,
    /// Wall rows in blocks, capped by the height the building was built to.
    total_h: i32,
    /// Wall cells, each with its texture column.
    cells: Vec<(i32, i32, u16)>,
}

/// What became of the candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlacementStats {
    /// Walls collected while the buildings went up.
    pub candidates: usize,
    /// Display entities written.
    pub displays: usize,
    /// Candidates that produced at least one display.
    pub placed: usize,
    /// Candidates that produced none.
    pub dropped: usize,
}

/// Panels of the world being generated, filled from the tile threads.
struct Registry {
    enabled: bool,
    px: u32,
    panels: Vec<Panel>,
    /// Walls already collected. A building is processed by every tile it
    /// overlaps, and a display claims no air cell that a second pass could
    /// find taken, so the duplicates have to be turned away by name.
    claimed: FnvHashSet<u32>,
    /// Candidates in collection order; `None` once settled.
    candidates: Vec<Option<Candidate>>,
    /// Candidate indices by the 512-block regions their cells touch, so a
    /// region can be settled before stream-to-disk eviction drops it.
    by_region: FnvHashMap<(i32, i32), Vec<usize>>,
    stats: PlacementStats,
}

/// Replaced on every generation, like the facade store: the GUI generates
/// several worlds in one process.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    enabled: false,
    px: 16,
    panels: Vec::new(),
    claimed: FnvHashSet::with_hasher(fnv::FnvBuildHasher::new()),
    candidates: Vec::new(),
    by_region: FnvHashMap::with_hasher(fnv::FnvBuildHasher::new()),
    stats: PlacementStats {
        candidates: 0,
        displays: 0,
        placed: 0,
        dropped: 0,
    },
});

/// Starts a generation: forgets the previous world's panels. `px` is the
/// requested texture resolution in pixels per block.
pub fn reset(enabled: bool, px: u32) {
    let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    *r = Registry {
        enabled,
        px,
        panels: Vec::new(),
        claimed: FnvHashSet::default(),
        candidates: Vec::new(),
        by_region: FnvHashMap::default(),
        stats: PlacementStats::default(),
    };
}

/// The outcome so far, for tests.
#[cfg(test)]
fn stats() -> PlacementStats {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).stats
}

fn warn(msg: &str) {
    eprintln!("Warning: {msg}");
    // -1 leaves the progress bar alone and only shows the text.
    emit_gui_progress_update(MESSAGE_ONLY, msg);
}

/// Records one candidate per textured wall of the building built under
/// `element_id`. Runs after the wall ring is built; nothing is written until
/// `finalize`, so every quad is placed against the finished world from the
/// main editor rather than from whichever tile happened to build the wall.
/// Returns the number of candidates recorded.
pub fn collect(
    editor: &mut WorldEditor,
    element_id: u64,
    start_y_offset: i32,
    abs_terrain_offset: i32,
    building_height: i32,
) -> usize {
    let Some(s) = facades::store() else {
        return 0;
    };
    if !s.displays || !editor.map_decals_enabled() {
        return 0;
    }
    let Some(columns) = s.way_cells.get(&element_id) else {
        return 0;
    };

    // Cells per wall. A cell two pieces of one wall both landed on belongs to
    // the last one, the same rule `block_at` follows, and the ring walk can
    // hand the same cell over twice.
    let mut by_wall: BTreeMap<u32, WallCells> = BTreeMap::new();
    let mut seen = FnvHashSet::default();
    for &(bx, bz) in columns {
        if !seen.insert((bx, bz)) {
            continue;
        }
        let Some(cell) = s.cells.get(&(bx, bz)) else {
            continue;
        };
        let wall = &s.walls[cell.wall as usize];
        if wall.way_id != element_id || wall.tex.is_none() {
            continue;
        }
        let entry = by_wall.entry(cell.wall).or_default();
        entry.cells.push((bx, bz, cell.col));
        entry.snapped.0 += i32::from(cell.nx);
        entry.snapped.1 += i32::from(cell.nz);
    }
    if by_wall.is_empty() {
        return 0;
    }

    let scale = s.scale;
    let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    if !registry.enabled {
        return 0;
    }
    // Absolute y of the first wall block, the row `block_at` calls row 0.
    let base_y = start_y_offset + 1 + abs_terrain_offset;
    let mut collected = 0;

    for (wi, wall_cells) in by_wall {
        if !registry.claimed.insert(wi) {
            continue;
        }
        let WallCells { cells, snapped } = wall_cells;
        let wall = &s.walls[wi as usize];
        // The textured rows, in blocks, capped by the wall that was built.
        let total_h = ((f64::from(wall.rows) * scale).round() as i32).min(building_height);
        if total_h <= 0 {
            continue;
        }
        // The quad hangs just outside the wall, so the cell in front of each
        // wall cell counts towards the regions this candidate has to beat.
        let (ox, oz) = (snapped.0.signum(), snapped.1.signum());
        let mut regions: Vec<(i32, i32)> = Vec::with_capacity(2);
        for &(bx, bz, _) in &cells {
            for key in [(bx >> 9, bz >> 9), ((bx + ox) >> 9, (bz + oz) >> 9)] {
                if !regions.contains(&key) {
                    regions.push(key);
                }
            }
        }
        let index = registry.candidates.len();
        for key in regions {
            registry.by_region.entry(key).or_default().push(index);
        }
        registry.candidates.push(Some(Candidate {
            way_id: element_id,
            wall: wi,
            snapped,
            base_y,
            total_h,
            cells,
        }));
        registry.stats.candidates += 1;
        collected += 1;
    }
    collected
}

/// Name of one panel, unique because a wall is claimed once and each of its
/// pieces is named by where it starts. Resource paths take lowercase letters,
/// digits and underscores, and every part here is a non-negative number.
///
/// `prefix` separates the two facade sources, which number their walls
/// differently: `f` for a wall of the Mapillary export, `b` for a ring segment
/// of the preset facades. Without it one building could name two different
/// panels the same and the pack would carry only one of them.
fn panel_name(prefix: char, way_id: u64, wall: u32, along: i32, row: i32) -> String {
    format!("{prefix}{way_id}_{wall}_{along}_{row}")
}

/// UUID seed contribution of a panel. Two panels meeting at a corner can put
/// their centres in one block, and the block is all the entity UUID is
/// otherwise built from, so they would collide and the game would keep one.
fn name_seed(name: &str) -> i64 {
    let mut hash: i64 = 0x0102_0304_0506_0708;
    for byte in name.bytes() {
        hash ^= i64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The display's own NBT, on top of what `add_item_display` writes.
fn display_nbt(name: &str, quad: &Quad) -> HashMap<String, Value> {
    let floats = |v: &[f32]| Value::List(v.iter().map(|f| Value::Float(*f)).collect());

    let mut components = HashMap::new();
    components.insert(
        "minecraft:item_model".to_string(),
        Value::String(format!("{NAMESPACE}:{name}")),
    );
    let mut item = HashMap::new();
    item.insert("id".to_string(), Value::String(ITEM.to_string()));
    item.insert("count".to_string(), Value::Int(1));
    item.insert("components".to_string(), Value::Compound(components));

    // The model is a one-block quad centred on the entity, so the scale is the
    // panel's size in blocks outright. Display entities do not clamp it.
    let mut transformation = HashMap::new();
    transformation.insert("left_rotation".to_string(), floats(&quad.rot));
    transformation.insert("right_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0]));
    transformation.insert("translation".to_string(), floats(&[0.0, 0.0, 0.0]));
    transformation.insert(
        "scale".to_string(),
        floats(&[quad.w as f32, quad.h as f32, 1.0]),
    );

    let mut extra = HashMap::new();
    extra.insert("item".to_string(), Value::Compound(item));
    // `fixed` is the item frame transform: the model is drawn flat-on rather
    // than tilted the way a held or dropped item is.
    extra.insert(
        "item_display".to_string(),
        Value::String("fixed".to_string()),
    );
    extra.insert(
        "transformation".to_string(),
        Value::Compound(transformation),
    );
    // No billboarding: the quad keeps the angle the rotation gave it. `fixed`
    // still applies the entity's own yaw and pitch, which is why `Rotation`
    // stays at zero.
    extra.insert("billboard".to_string(), Value::String("fixed".to_string()));
    extra.insert("view_range".to_string(), Value::Float(VIEW_RANGE));
    // `width` and `height` are the culling box, which grows from the entity
    // position. Zero on either turns culling off outright, which is what a
    // panel scaled far beyond its own position needs: a box would blink the
    // quad out as soon as its centre point left the screen. Zero is also the
    // default and what the game writes back for its own displays, but it is
    // written out so a later reader does not have to look the default up.
    extra.insert("width".to_string(), Value::Float(0.0));
    extra.insert("height".to_string(), Value::Float(0.0));
    extra
}

/// Whether a wall shows above the ground it stands on. A wall buried in the
/// hillside shows nothing; the blocks are enough there.
pub(crate) fn wall_is_visible(
    editor: &mut WorldEditor,
    cells: impl IntoIterator<Item = (i32, i32)>,
    top: i32,
) -> bool {
    cells
        .into_iter()
        .any(|(bx, bz)| top > editor.get_absolute_y(bx, 0, bz) + 1)
}

/// Cuts `len` into pieces of at most `max`, as [start, end) ranges.
pub fn cut(len: i32, max: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < len {
        let end = (start + max).min(len);
        out.push((start, end));
        start = end;
    }
    out
}

/// Cuts one wall into pieces the block atlas can hold and hangs each as an
/// item display entity, asking `crop` for the pixels of each piece.
///
/// **Both facade sources come through here.** The Mapillary export and the
/// preset facades disagree about where their pixels come from and about
/// nothing else: the quad on the wall's true line, the entity NBT, the item
/// model and the resource pack are one mechanism, so they live in one place
/// and neither source can drift from the other.
///
/// `cells` are the wall's world columns, already sorted along the outside
/// viewer's right so that a piece's crop and its world position run the same
/// way. `crop(p0, p1, b0, b1)` is given the half-open cell range and wall row
/// range of a piece and returns its image, or `None` to leave that piece bare.
/// Returns how many panels were hung.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hang_wall(
    editor: &mut WorldEditor,
    prefix: char,
    way_id: u64,
    wall: u32,
    cells: &[(i32, i32)],
    n: (f64, f64),
    step: f64,
    base_y: i32,
    total_h: i32,
    crop: &mut dyn FnMut(i32, i32, i32, i32) -> Option<RgbImage>,
) -> usize {
    // Pieces of at most MAX_PANEL blocks of wall, which is fewer cells the
    // more the wall leans off its major axis.
    let max_cells = ((f64::from(MAX_PANEL) / step).floor() as i32).max(1);
    let mut hung = 0usize;
    for (p0, p1) in cut(cells.len() as i32, max_cells) {
        let footprint = &cells[p0 as usize..p1 as usize];
        for (b0, b1) in cut(total_h, MAX_PANEL) {
            let Some(tex) = crop(p0, p1, b0, b1) else {
                continue;
            };
            // The panel covers the wall the picture covers and no more, so the
            // bottom piece stops at the shell's first wall block. On a slope
            // the fill below it stays bare: the photograph has no pixels for
            // ground the camera never saw, and the block wall down there is
            // the building's own colour-matched material, which reads as a
            // plinth where invented pixels read as a smear.
            let quad = quad_for(footprint, n, step, base_y + b0, b1 - b0);
            let name = panel_name(prefix, way_id, wall, p0, b0);
            let nbt = display_nbt(&name, &quad);
            if !editor.add_item_display(quad.cx, quad.cy, quad.cz, name_seed(&name), nbt) {
                continue;
            }
            let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            if !r.enabled {
                return hung;
            }
            r.panels.push(Panel {
                name,
                w: quad.w,
                h: quad.h,
                tex,
            });
            hung += 1;
        }
    }
    hung
}

/// Places one collected wall: works out its quad, cuts it into pieces the
/// atlas can hold and writes one display entity per piece, each cropped out of
/// the 8 px/m wall texture. Returns how many pieces were hung.
fn place_candidate(editor: &mut WorldEditor, s: &FacadeStore, cand: Candidate) -> usize {
    let wall = &s.walls[cand.wall as usize];
    let dir = s.wall_dir[cand.wall as usize];
    let (Some(tex), Some(n)) = (
        wall.tex.as_ref(),
        outward_normal(dir, cand.snapped.0, cand.snapped.1),
    ) else {
        return 0;
    };

    let top = cand.base_y + cand.total_h;
    if !wall_is_visible(editor, cand.cells.iter().map(|&(bx, bz, _)| (bx, bz)), top) {
        return 0;
    }

    // The outside viewer's left first, so a piece's crop and its world
    // position run the same way.
    let (rx, rz) = right_of(n);
    let mut cells = cand.cells;
    cells.sort_by(|a, b| {
        let along = |c: &(i32, i32, u16)| f64::from(c.0) * rx + f64::from(c.1) * rz;
        along(a).total_cmp(&along(b))
    });

    let step = cell_step(dir);
    let flip = flip_crop(dir, n);
    let fallback = s.building_colour(cand.way_id);
    let scale = s.scale;
    let footprints: Vec<(i32, i32)> = cells.iter().map(|c| (c.0, c.1)).collect();
    let rows = f64::from(wall.rows);

    let mut crop = |p0: i32, p1: i32, b0: i32, b1: i32| {
        // The piece covers whole texture columns. Two pieces can share or skip
        // one metre at their seam, where the Bresenham walk doubled a column
        // or stepped over one; that is a sub-metre seam every 32 blocks.
        let (cmin, cmax) = cells[p0 as usize..p1 as usize]
            .iter()
            .fold((u16::MAX, 0u16), |(a, b), c| (a.min(c.2), b.max(c.2)));
        // Metres down from the top of the texture.
        let va = rows - f64::from(b1) / scale;
        let vb = rows - f64::from(b0) / scale;
        crop_texture(
            tex,
            f64::from(cmin),
            f64::from(cmax) + 1.0,
            va,
            vb,
            flip,
            fallback,
        )
    };
    hang_wall(
        editor,
        'f',
        cand.way_id,
        cand.wall,
        &footprints,
        n,
        step,
        cand.base_y,
        cand.total_h,
        &mut crop,
    )
}

/// Crops metres [ua, ub) along the wall and [va, vb) down from the top out of
/// the 8 px/m wall texture, mirrors it when asked and fills missing pixels
/// with the wall colour around them (`fill_missing`, with `fallback` as the
/// last resort). None when too little of the crop carries texture.
fn crop_texture(
    tex: &RgbaImage,
    ua: f64,
    ub: f64,
    va: f64,
    vb: f64,
    flip: bool,
    fallback: Option<RGBTuple>,
) -> Option<RgbImage> {
    let (tw, th) = (i64::from(tex.width()), i64::from(tex.height()));
    if tw == 0 || th == 0 {
        return None;
    }
    let x0 = ((ua * TEX_PX_PER_M).floor() as i64).clamp(0, tw - 1);
    let x1 = ((ub * TEX_PX_PER_M).ceil() as i64).clamp(x0 + 1, tw);
    let y0 = ((va * TEX_PX_PER_M).floor() as i64).clamp(0, th - 1);
    let y1 = ((vb * TEX_PX_PER_M).ceil() as i64).clamp(y0 + 1, th);
    let crop = image::imageops::crop_imm(
        tex,
        x0 as u32,
        y0 as u32,
        (x1 - x0) as u32,
        (y1 - y0) as u32,
    )
    .to_image();
    let mut rgb = fill_missing(&crop, fallback)?;
    if flip {
        image::imageops::flip_horizontal_in_place(&mut rgb);
    }
    Some(rgb)
}

/// Fills the alpha-0 pixels so a hole reads as flat wall, not as a streak,
/// and drops the alpha. A missing pixel takes the median colour of the valid
/// pixels in its own texture row of the crop (one row is one height on the
/// facade, so that is the wall around the hole), the crop's valid median
/// when its row has none, and `fallback`, the lab's building colour, after
/// that. The filled pixels then get a 3 px box blur so the seam to the
/// photograph does not cut; the photograph itself is left alone. None when
/// fewer than `MIN_VALID_FRACTION` of the pixels were valid.
fn fill_missing(crop: &RgbaImage, fallback: Option<RGBTuple>) -> Option<RgbImage> {
    let (w, h) = (crop.width() as usize, crop.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let rgb: Vec<[u8; 3]> = crop.pixels().map(|p| [p[0], p[1], p[2]]).collect();
    let valid: Vec<bool> = crop.pixels().map(|p| p[3] > 0).collect();
    let valid_count = valid.iter().filter(|ok| **ok).count();
    if valid_count == 0 || (valid_count as f64) < MIN_VALID_FRACTION * (w * h) as f64 {
        return None;
    }
    let to_image = |px: Vec<[u8; 3]>| {
        RgbImage::from_raw(w as u32, h as u32, px.into_iter().flatten().collect())
    };
    if valid_count == w * h {
        return to_image(rgb);
    }

    let valid_pixels = |px: &[[u8; 3]], ok: &[bool]| -> Vec<[u8; 3]> {
        px.iter()
            .zip(ok)
            .filter(|(_, ok)| **ok)
            .map(|(p, _)| *p)
            .collect()
    };
    let crop_median = median_colour(&valid_pixels(&rgb, &valid))
        .or(fallback.map(|(r, g, b)| [r, g, b]))
        .unwrap_or([128, 128, 128]);
    let mut filled = rgb.clone();
    for y in 0..h {
        let (row, ok) = (&rgb[y * w..(y + 1) * w], &valid[y * w..(y + 1) * w]);
        if ok.iter().all(|v| *v) {
            continue;
        }
        let colour = median_colour(&valid_pixels(row, ok)).unwrap_or(crop_median);
        for x in 0..w {
            if !ok[x] {
                filled[y * w + x] = colour;
            }
        }
    }

    // The blur reads the filled image and writes the filled pixels only.
    let mut out = filled.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if valid[i] {
                continue;
            }
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for sy in y.saturating_sub(1)..(y + 2).min(h) {
                for sx in x.saturating_sub(1)..(x + 2).min(w) {
                    for (a, c) in acc.iter_mut().zip(filled[sy * w + sx]) {
                        *a += u32::from(c);
                    }
                    n += 1;
                }
            }
            out[i] = acc.map(|a| (a / n) as u8);
        }
    }
    to_image(out)
}

/// Per-channel median of `pixels`; None when there are none.
fn median_colour(pixels: &[[u8; 3]]) -> Option<[u8; 3]> {
    if pixels.is_empty() {
        return None;
    }
    let mut channels: [Vec<u8>; 3] = Default::default();
    for px in pixels {
        for (c, v) in channels.iter_mut().zip(px) {
            c.push(*v);
        }
    }
    Some(channels.map(|mut c| {
        c.sort_unstable();
        c[c.len() / 2]
    }))
}

/// Settles the candidates at `indices` that are still pending.
fn place_pending(editor: &mut WorldEditor, indices: impl IntoIterator<Item = usize>) {
    let Some(s) = facades::store() else {
        return;
    };
    for i in indices {
        // The registry lock is taken around the bookkeeping and released
        // across the placement, because `hang_wall` takes it once per panel to
        // push it and a re-entrant lock would deadlock.
        let cand = {
            let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            if !r.enabled {
                return;
            }
            match r.candidates.get_mut(i).and_then(Option::take) {
                Some(cand) => cand,
                None => continue,
            }
        };
        let hung = place_candidate(editor, &s, cand);
        let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        r.stats.displays += hung;
        if hung == 0 {
            r.stats.dropped += 1;
        } else {
            r.stats.placed += 1;
        }
    }
}

/// Settles the pending candidates whose cells touch region `(rx, rz)`. Under
/// stream-to-disk eviction an entity written after a region was flushed is
/// lost, so this runs right before the flush.
pub fn flush_region(editor: &mut WorldEditor, rx: i32, rz: i32) {
    let indices: Vec<usize> = {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled {
            return;
        }
        r.by_region.get(&(rx, rz)).cloned().unwrap_or_default()
    };
    if !indices.is_empty() {
        place_pending(editor, indices);
    }
}

/// Settles every candidate still pending against the finished world, right
/// before it is saved, and reports the outcome to the GUI. `None` when the run
/// collected nothing.
pub fn finalize(editor: &mut WorldEditor) -> Option<PlacementReport> {
    let count = {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled || r.stats.candidates == 0 {
            return None;
        }
        r.candidates.len()
    };
    place_pending(editor, 0..count);
    let report = PlacementReport {
        stats: REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).stats,
    };
    emit_gui_progress_update(MESSAGE_ONLY, &report.summary());
    Some(report)
}

/// Summary of what `finalize` wrote.
pub struct PlacementReport {
    pub stats: PlacementStats,
}

impl PlacementReport {
    fn summary(&self) -> String {
        let s = self.stats;
        format!(
            "Facade panels: {} display entities on {} of {} walls ({} without a usable crop)",
            s.displays, s.placed, s.candidates, s.dropped
        )
    }
}

impl fmt::Display for PlacementReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "  {}", self.summary())
    }
}

/// The item model definition an item stack's `minecraft:item_model` component
/// resolves to (`assets/<ns>/items/<name>.json`, 1.21.4+).
pub fn item_definition_json(name: &str) -> String {
    serde_json::json!({
        "model": {
            "type": "minecraft:model",
            "model": format!("{NAMESPACE}:item/{name}")
        }
    })
    .to_string()
}

/// The model itself: one flat element in the middle of the block, textured on
/// both of its wide faces.
///
/// No parent. `minecraft:block/block` would look like the natural one, but its
/// `fixed` display transform scales to half a block and would silently halve
/// every panel; without a parent every transform is the identity, which is
/// what a quad placed by its entity wants.
///
/// The element spans the whole 16 by 16 of the model in x and y and is 0.2
/// thick around z = 8, so its centre is the model's centre and the display
/// entity's position is the middle of the photograph. Its wide faces are
/// `north` (-z) and `south` (+z); `uv` [0, 0, 16, 16] is the identity mapping
/// on both, which reads upright and unmirrored to a viewer of either, the same
/// way one texture on `block/cube_all` reads on every side of a block.
///
/// Both faces have to be textured. The renderer turns the model half a circle
/// about +y before drawing it (`left_rotation` documents why), so it is
/// `north` that faces the street, and identity `uv` on a `north` face runs its
/// u to the right of a viewer standing outside it, exactly as identity `uv` on
/// a `south` face does for a viewer of that one. The half turn and the swapped
/// face cancel, which is why the crop the outside viewer sees is the one
/// `right_of` laid out and not its mirror.
pub fn model_json(name: &str) -> String {
    model_json_for(name)
}

/// The model for a panel whose picture lives under `texture_name`, which is the
/// panel's own name unless an identical panel got there first.
pub fn model_json_for(texture_name: &str) -> String {
    let texture = format!("{NAMESPACE}:block/{texture_name}");
    serde_json::json!({
        "textures": { "0": texture, "particle": texture },
        "elements": [{
            "from": [0.0, 0.0, 7.9],
            "to": [16.0, 16.0, 8.1],
            "faces": {
                "north": { "uv": [0, 0, 16, 16], "texture": "#0" },
                "south": { "uv": [0, 0, 16, 16], "texture": "#0" }
            }
        }],
        "display": {
            "fixed": {
                "rotation": [0, 0, 0],
                "translation": [0, 0, 0],
                "scale": [1, 1, 1]
            }
        }
    })
    .to_string()
}

/// Texture side in pixels for `blocks` blocks at `px` pixels per block,
/// rounded to a whole multiple of 16 and at least 16.
///
/// The model maps the whole texture onto the whole quad, so this only chooses
/// the resolution and can never stretch the photograph. The multiple of 16 is
/// what the block atlas wants: it is mipmapped four levels deep, and one
/// texture that cannot be halved four times costs every block in the world
/// its mipmaps.
fn tex_side(blocks: f64, px: u32) -> u32 {
    let raw = blocks * f64::from(px) / 16.0;
    (raw.round() as u32).max(1) * 16
}

/// Summary of what `write_packs` produced.
pub struct PackReport {
    pub panels: usize,
    pub px: u32,
    pub requested_px: u32,
}

impl fmt::Display for PackReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "  Facade panels: {} item display panels at {} px per block in resources.zip",
            self.panels, self.px
        )?;
        if self.px < self.requested_px {
            write!(
                f,
                " (lowered from {} px to fit the atlas)",
                self.requested_px
            )?;
        }
        Ok(())
    }
}

/// Writes the placed panels as the world's resource pack. No data pack: the
/// item model definitions are resource pack files. `Ok(None)` when the run
/// placed nothing.
pub fn write_packs(world_path: &Path) -> Result<Option<PackReport>, String> {
    let (panels, px) = {
        let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled || r.panels.is_empty() {
            return Ok(None);
        }
        (std::mem::take(&mut r.panels), r.px)
    };
    emit_gui_progress_update(98.0, "Writing facade panels...");
    write_packs_for(world_path, panels, px).map(Some)
}

/// Drops the panels of a run that will not reach [`write_packs`].
///
/// The registry lives for the process, and a city's crops are a few hundred
/// megabytes that would otherwise sit there from a failed save until the next
/// generation resets it.
pub fn discard() {
    let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    r.panels = Vec::new();
}

/// Total panel texture area at `px` pixels per block.
/// What makes two panels the same picture: the pixels, and the size they are
/// written at. A city hangs the same facade on hundreds of walls, so most
/// panels are byte identical to another one.
fn pixel_hash(p: &Panel) -> u64 {
    use std::hash::Hasher;
    let mut h = fnv::FnvHasher::default();
    h.write(p.tex.as_raw());
    h.write_u32(p.tex.width());
    h.write_u32(p.tex.height());
    h.finish()
}

/// The hash of every panel's pixels, in panel order.
///
/// Hashing a city's panels is a pass over a couple of hundred megabytes, and
/// what comes out does not depend on the resolution the pack ends up at, so it
/// is taken once here instead of once per `fit_px` step and once more while
/// the zip is written.
fn pixel_hashes(panels: &[Panel]) -> Vec<u64> {
    panels.par_iter().map(pixel_hash).collect()
}

fn panel_key(p: &Panel, pixels: u64, px: u32) -> (u64, u32, u32) {
    (pixels, tex_side(p.w, px), tex_side(p.h, px))
}

/// `atlas_area` from the panels alone, for tests that are about the budget and
/// not about who hashes what.
#[cfg(test)]
fn atlas_area_of(panels: &[Panel], px: u32) -> u64 {
    atlas_area(panels, &pixel_hashes(panels), px)
}

/// `fit_px` from the panels alone, for the same reason.
#[cfg(test)]
fn fit_px_of(panels: &[Panel], requested_px: u32) -> u32 {
    fit_px(panels, &pixel_hashes(panels), requested_px)
}

/// Atlas pixels the pack costs, counting each distinct picture once.
///
/// The game stitches one texture per file, so two panels sharing a file cost
/// the atlas one entry, not two. Counting per panel made a small town look like
/// it needed the whole budget and pushed `fit_px` down a step or two for
/// nothing, which is where the panels lost their sharpness.
fn atlas_area(panels: &[Panel], hashes: &[u64], px: u32) -> u64 {
    let mut seen: FnvHashSet<(u64, u32, u32)> = FnvHashSet::default();
    panels
        .iter()
        .zip(hashes)
        .filter(|(p, &pixels)| seen.insert(panel_key(p, pixels, px)))
        .map(|(p, _)| u64::from(tex_side(p.w, px)) * u64::from(tex_side(p.h, px)))
        .sum()
}

/// Halves the resolution until the atlas budget holds, down to 4 px per block.
fn fit_px(panels: &[Panel], hashes: &[u64], requested_px: u32) -> u32 {
    let mut px = requested_px.max(MIN_PX_PER_BLOCK);
    // One step at a time, not `px /= 2`: halving is a four times jump in
    // area, and on a real box it lands at 8 where 11 would have fitted. The
    // atlas only cares that a sprite side is a multiple of 16, which
    // `tex_side` gives for any integer px, so nothing here wants a power of
    // two.
    while px > MIN_PX_PER_BLOCK && atlas_area(panels, hashes, px) > atlas_budget() {
        px -= 1;
    }
    px
}

/// pack.mcmeta accepted by every 1.21.x and later: the old keys for 1.21 to
/// 1.21.8, `min_format`/`max_format` for 1.21.9+, each open-ended upwards.
fn pack_mcmeta(pack_format: u32) -> String {
    serde_json::json!({
        "pack": {
            "pack_format": pack_format,
            "supported_formats": [pack_format, 999],
            "min_format": pack_format,
            "max_format": [999, 0],
            "description": "Arnis facade panels"
        }
    })
    .to_string()
}

/// Writes the resource pack into `out`: pack.mcmeta, the marker, an item
/// definition and a model per panel, and one texture per distinct picture.
///
/// The pictures are resized and PNG encoded in parallel, a chunk at a time,
/// and every crop leaves the panel list before that starts. On a city the
/// serial encode was nine tenths of the time the panels added to a run, and
/// the crops held through it were most of the memory.
fn write_resource_zip<W: Write + Seek>(
    out: W,
    mut panels: Vec<(Panel, u64)>,
    px: u32,
) -> Result<(), String> {
    let mut zip = zip::ZipWriter::new(out);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    // A PNG carries deflated pixels already, so deflating it a second time
    // shrinks the pack by a few per cent and costs more than everything else
    // the writer does put together.
    let png_options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut put =
        |path: String, bytes: &[u8], opts: zip::write::FileOptions| -> Result<(), String> {
            zip.start_file(path, opts).map_err(|e| e.to_string())?;
            zip.write_all(bytes).map_err(|e| e.to_string())
        };
    put(
        "pack.mcmeta".to_string(),
        pack_mcmeta(RESOURCEPACK_FORMAT).as_bytes(),
        options,
    )?;
    put(PACK_MARKER.to_string(), b"arnis facade panels\n", options)?;

    // The definitions and models, and which panel owns each distinct picture.
    let mut texture_of: FnvHashMap<(u64, u32, u32), String> = FnvHashMap::default();
    let mut owners: Vec<usize> = Vec::new();
    for (i, (p, pixels)) in panels.iter().enumerate() {
        put(
            format!("assets/{NAMESPACE}/items/{}.json", p.name),
            item_definition_json(&p.name).as_bytes(),
            options,
        )?;
        let key = panel_key(p, *pixels, px);
        if let Some(shared) = texture_of.get(&key) {
            // An identical panel already owns this picture; point at it.
            put(
                format!("assets/{NAMESPACE}/models/item/{}.json", p.name),
                model_json_for(shared).as_bytes(),
                options,
            )?;
            continue;
        }
        texture_of.insert(key, p.name.clone());
        put(
            format!("assets/{NAMESPACE}/models/item/{}.json", p.name),
            model_json(&p.name).as_bytes(),
            options,
        )?;
        owners.push(i);
    }

    // The pictures. Every crop leaves the list here, the owners into the queue
    // and the duplicates straight to the drop, so from now on the run holds
    // the crops still to be written plus one chunk of PNGs, not the crops
    // plus the whole pack.
    let mut queue: Vec<(String, RgbImage, u32, u32)> = Vec::with_capacity(owners.len());
    for i in owners {
        let (p, _) = &mut panels[i];
        let tex = std::mem::replace(&mut p.tex, RgbImage::new(0, 0));
        queue.push((p.name.clone(), tex, tex_side(p.w, px), tex_side(p.h, px)));
    }
    drop(panels);
    // Chunked so the encoded bytes waiting for the zip stay a few megabytes.
    const CHUNK: usize = 64;
    while !queue.is_empty() {
        let n = queue.len().min(CHUNK);
        let batch: Vec<_> = queue.drain(..n).collect();
        let encoded: Vec<Result<(String, Vec<u8>), String>> = batch
            .into_par_iter()
            .map(|(name, tex, w, h)| {
                // A crop already at the pack's size is encoded as it is
                // rather than copied first.
                let img = if tex.width() == w && tex.height() == h {
                    tex
                } else {
                    image::imageops::resize(&tex, w, h, image::imageops::FilterType::Triangle)
                };
                let mut png = Vec::new();
                img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                    .map_err(|e| format!("encode {name}: {e}"))?;
                Ok((name, png))
            })
            .collect();
        for item in encoded {
            let (name, png) = item?;
            put(
                format!("assets/{NAMESPACE}/textures/block/{name}.png"),
                &png,
                png_options,
            )?;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// The entry that marks a world pack as ours.
///
/// A file, not a word in the description: the description was matched on the
/// substring "Arnis", so a user's own pack that merely mentioned Arnis in its
/// description was taken for ours and replaced. Nothing but this writer puts
/// this path in a pack.
pub(super) const PACK_MARKER: &str = ".arnis_facade_pack";

/// Whether the pack at `path` is one Arnis wrote.
///
/// Both facade sources install the world pack at the two names the game reads,
/// and `fs::write` would replace whatever is there. A world Arnis generated
/// holds its own pack and replacing that is the point; a world the user has
/// dressed themselves holds theirs, and losing it to a regeneration is not
/// something they can undo. Ours says so in `pack.mcmeta`.
pub(super) fn is_arnis_pack(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return false;
    };
    (0..zip.len()).any(|i| {
        zip.by_index_raw(i)
            .map(|f| f.name() == PACK_MARKER)
            .unwrap_or(false)
    })
}

/// Installs the staged pack file at `path`, stepping a pack we did not write
/// aside first rather than overwriting it. The backup is numbered so a second
/// generation cannot bury the first one's rescue. The staged file is a sibling
/// of `path`, so the last step is one rename: a run that dies half way leaves
/// the world with the pack it had, not a truncated zip that the next run would
/// take for someone else's and move aside.
pub(super) fn install_world_pack(path: &Path, staged: &Path) -> Result<(), String> {
    if path.exists() && !is_arnis_pack(path) {
        let mut backup = path.with_extension("zip.bak");
        let mut n = 1;
        while backup.exists() {
            backup = path.with_extension(format!("zip.bak{n}"));
            n += 1;
        }
        std::fs::rename(path, &backup)
            .map_err(|e| format!("move {} aside: {e}", path.display()))?;
        warn(&format!(
            "Facade panels: this world already had a resource pack Arnis did not write; it is kept at {}.",
            backup.display()
        ));
    }
    std::fs::rename(staged, path).map_err(|e| format!("install {}: {e}", path.display()))
}

/// [`install_world_pack`] for bytes already in memory.
#[cfg(test)]
pub(super) fn write_world_pack(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let staged = path.with_extension("zip.tmp");
    std::fs::write(&staged, bytes).map_err(|e| format!("write {}: {e}", staged.display()))?;
    install_world_pack(path, &staged)
}

/// Writes the resource pack for `panels` into `world_path`, at `requested_px`
/// pixels per block or the highest resolution below it that fits the atlas.
fn write_packs_for(
    world_path: &Path,
    panels: Vec<Panel>,
    requested_px: u32,
) -> Result<PackReport, String> {
    let hashes = pixel_hashes(&panels);
    let px = fit_px(&panels, &hashes, requested_px);
    if px < requested_px {
        warn(&format!(
            "Facade panels: {} panels would not fit the game's block atlas at {requested_px} px per block; using {px} px.",
            panels.len()
        ));
    }
    if atlas_area(&panels, &hashes, px) > atlas_budget() {
        warn(&format!(
            "Facade panels: {} panels exceed the block atlas even at {px} px per block; the game may fail to stitch them.",
            panels.len()
        ));
    }
    let count = panels.len();
    let mut panels: Vec<(Panel, u64)> = panels.into_iter().zip(hashes).collect();
    panels.sort_by(|a, b| a.0.name.cmp(&b.0.name));

    let rp_dir = world_path.join("resourcepacks");
    std::fs::create_dir_all(&rp_dir).map_err(|e| format!("create {}: {e}", rp_dir.display()))?;
    let primary = world_path.join("resources.zip");
    let secondary = rp_dir.join("resources.zip");

    // Streamed to disk beside its final name: assembled in memory the zip was
    // as large again as the crops on a city. The game has looked in two places
    // for a world's pack, so the finished file is copied to the other one.
    let staged = primary.with_extension("zip.tmp");
    let written = (|| -> Result<(), String> {
        let file = std::fs::File::create(&staged)
            .map_err(|e| format!("create {}: {e}", staged.display()))?;
        let mut out = BufWriter::new(file);
        write_resource_zip(&mut out, panels, px)?;
        out.flush()
            .map_err(|e| format!("write {}: {e}", staged.display()))
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    install_world_pack(&primary, &staged)?;
    let staged = secondary.with_extension("zip.tmp");
    std::fs::copy(&primary, &staged).map_err(|e| format!("copy {}: {e}", staged.display()))?;
    install_world_pack(&secondary, &staged)?;

    Ok(PackReport {
        panels: count,
        px,
        requested_px,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::mapillary::facades::{CellRef, FacadeWall, TEST_GLOBALS as GLOBALS};
    use image::Rgb;
    use std::path::PathBuf;

    /// Unit normals of the four cardinal walls plus one at 45 degrees.
    const ROOT_HALF: f64 = std::f64::consts::FRAC_1_SQRT_2;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn quat_close(got: [f32; 4], want: [f64; 4]) -> bool {
        got.iter()
            .zip(want)
            .all(|(g, w)| (f64::from(*g) - w).abs() < 1e-6)
    }

    #[test]
    fn a_wall_turns_its_quad_onto_its_own_normal() {
        // dir is node A to node B, (snx, snz) the axis-snapped outward normal
        // the projection recorded; the true normal is the perpendicular of dir
        // on that side. Yaw 0 is south, +90 west.
        // (node A to node B, snapped normal, true normal, yaw in degrees)
        type Case = ((i32, i32), (i32, i32), (f64, f64), f64);
        let cases: [Case; 5] = [
            // A wall running east with the building to the north: faces south.
            ((19, 0), (0, 1), (0.0, 1.0), 0.0),
            // Running south with the building to the east: faces west.
            ((0, 19), (-1, 0), (-1.0, 0.0), 90.0),
            // Running west with the building to the south: faces north.
            ((-19, 0), (0, -1), (0.0, -1.0), 180.0),
            // Running north with the building to the west: faces east.
            ((0, -19), (1, 0), (1.0, 0.0), -90.0),
            // 45 degrees to the south-east with the building south-west: the
            // snap picked east, the true normal is north-east.
            ((7, 7), (1, 0), (ROOT_HALF, -ROOT_HALF), -135.0),
        ];
        for (dir, snap, want_n, want_yaw) in cases {
            let n = outward_normal(dir, snap.0, snap.1).unwrap();
            assert!(
                close(n.0, want_n.0) && close(n.1, want_n.1),
                "dir {dir:?}: normal {n:?} wanted {want_n:?}"
            );
            assert!(
                close(yaw_deg(n), want_yaw),
                "dir {dir:?}: yaw {}",
                yaw_deg(n)
            );
            // The quaternion has to turn the model's own +z face onto it.
            let facing = facing_of(left_rotation(n));
            assert!(
                close(facing.0, want_n.0) && close(facing.1, want_n.1),
                "dir {dir:?}: facing {facing:?} wanted {want_n:?}"
            );
        }

        // The quaternions themselves, as [x, y, z, w] with y = sin(theta / 2)
        // for a turn of theta = -yaw about +y.
        assert!(quat_close(left_rotation((0.0, 1.0)), [0.0, 0.0, 0.0, 1.0]));
        assert!(quat_close(
            left_rotation((-1.0, 0.0)),
            [0.0, -ROOT_HALF, 0.0, ROOT_HALF]
        ));
        assert!(quat_close(left_rotation((0.0, -1.0)), [0.0, 1.0, 0.0, 0.0]));
        assert!(quat_close(
            left_rotation((1.0, 0.0)),
            [0.0, ROOT_HALF, 0.0, ROOT_HALF]
        ));
        // 135 degrees about +y: sin(67.5) and cos(67.5).
        assert!(quat_close(
            left_rotation((ROOT_HALF, -ROOT_HALF)),
            [0.0, 0.923_879_532_5, 0.0, 0.382_683_432_4]
        ));

        // The viewer's right on each axis normal, standing outside the wall.
        assert_eq!(right_of((0.0, 1.0)), (1.0, 0.0), "facing south, right east");
        assert_eq!(
            right_of((-1.0, 0.0)),
            (0.0, 1.0),
            "facing west, right south"
        );
        assert_eq!(
            right_of((0.0, -1.0)),
            (-1.0, 0.0),
            "facing north, right west"
        );
        assert_eq!(
            right_of((1.0, 0.0)),
            (0.0, -1.0),
            "facing east, right north"
        );

        // A wall with no direction at all falls back on the snap.
        assert_eq!(outward_normal((0, 0), 0, 1), Some((0.0, 1.0)));
        assert_eq!(outward_normal((0, 0), 0, 0), None);
    }

    #[test]
    fn one_quad_spans_the_whole_wall_and_clears_its_blocks() {
        // A 20 block wall along x at z = 50, twelve rows high, facing south.
        let cells: Vec<(i32, i32)> = (0..20).map(|i| (100 + i, 50)).collect();
        let n = (0.0, 1.0);
        let quad = quad_for(&cells, n, 1.0, 64, 12);
        assert!(close(quad.w, 20.0) && close(quad.h, 12.0));
        assert!(close(quad.cx, 110.0), "centred along the wall");
        // The blocks fill z 50 to 51; the quad sits PUSH_OUT in front of that.
        assert!(close(quad.cz, 51.0 + PUSH_OUT));
        assert!(close(quad.cy, 70.0), "bottom at 64, twelve blocks up");
        assert!(quat_close(quad.rot, [0.0, 0.0, 0.0, 1.0]));

        // The same wall seen from the north: the quad flips to the other side.
        let quad = quad_for(&cells, (0.0, -1.0), 1.0, 64, 12);
        assert!(close(quad.cx, 110.0) && close(quad.cz, 50.0 - PUSH_OUT));
        assert!(close(quad.w, 20.0));

        // A staircase at 45 degrees: one quad on the true line, not eight
        // panels. Its plane clears the outer corner of every step, which
        // sticks out further than half a block.
        let steps: Vec<(i32, i32)> = (0..8).map(|i| (100 + i, 50 + i)).collect();
        let n = (ROOT_HALF, -ROOT_HALF);
        let step = cell_step((7, 7));
        let quad = quad_for(&steps, n, step, 64, 6);
        assert!(close(quad.w, 98.0f64.sqrt() + step), "w = {}", quad.w);
        // Centre of the cells, pushed out along the normal by the corner
        // reach (one over root two) plus PUSH_OUT.
        let out = ROOT_HALF + PUSH_OUT;
        assert!(close(quad.cx, 104.0 + out * ROOT_HALF), "cx = {}", quad.cx);
        assert!(close(quad.cz, 54.0 - out * ROOT_HALF), "cz = {}", quad.cz);
    }

    #[test]
    fn a_cell_carries_more_wall_the_further_the_wall_leans() {
        assert!(close(cell_step((19, 0)), 1.0));
        assert!(close(cell_step((0, -19)), 1.0));
        assert!(close(cell_step((7, 7)), std::f64::consts::SQRT_2));
        assert!(close(cell_step((20, 3)), 409.0f64.sqrt() / 20.0));
        assert!(close(cell_step((0, 0)), 1.0));
    }

    #[test]
    fn crop_is_mirrored_when_node_a_is_on_the_viewers_right() {
        // The texture's columns run from node A to node B. Seen from the side
        // where that runs left to right the crop is used as it is; from the
        // other side it has to be mirrored, or the facade reads backwards.
        assert!(
            !flip_crop((10, 0), (0.0, 1.0)),
            "facing south, right is east"
        );
        assert!(
            flip_crop((10, 0), (0.0, -1.0)),
            "facing north, right is west"
        );
        assert!(
            flip_crop((0, 10), (1.0, 0.0)),
            "facing east, right is north"
        );
        assert!(
            !flip_crop((0, 10), (-1.0, 0.0)),
            "facing west, right is south"
        );
        // At 45 degrees the outward normal decides it just the same.
        assert!(flip_crop((7, 7), (ROOT_HALF, -ROOT_HALF)));
        assert!(!flip_crop((7, 7), (-ROOT_HALF, ROOT_HALF)));
    }

    #[test]
    fn the_model_and_its_definition_have_the_1_21_4_shape() {
        let v: serde_json::Value = serde_json::from_str(&item_definition_json("f1_0_0_0")).unwrap();
        assert_eq!(v["model"]["type"], "minecraft:model");
        assert_eq!(v["model"]["model"], "arnis:item/f1_0_0_0");

        let v: serde_json::Value = serde_json::from_str(&model_json("f1_0_0_0")).unwrap();
        // block/block would halve the quad through its own `fixed` transform.
        assert!(v.get("parent").is_none(), "the model must have no parent");
        assert_eq!(v["textures"]["0"], "arnis:block/f1_0_0_0");
        assert_eq!(v["textures"]["particle"], "arnis:block/f1_0_0_0");
        assert_eq!(v["display"]["fixed"]["scale"], serde_json::json!([1, 1, 1]));
        assert_eq!(
            v["display"]["fixed"]["rotation"],
            serde_json::json!([0, 0, 0])
        );
        let element = &v["elements"][0];
        assert_eq!(element["from"], serde_json::json!([0.0, 0.0, 7.9]));
        assert_eq!(element["to"], serde_json::json!([16.0, 16.0, 8.1]));
        // Both wide faces carry the identity mapping, so the photograph reads
        // upright and unmirrored whichever side the viewer stands on.
        for face in ["north", "south"] {
            assert_eq!(
                element["faces"][face]["uv"],
                serde_json::json!([0, 0, 16, 16]),
                "{face}"
            );
            assert_eq!(element["faces"][face]["texture"], "#0", "{face}");
        }
        assert!(element["faces"].get("east").is_none());
    }

    #[test]
    fn texture_sides_stay_mipmappable_and_the_ladder_finds_the_largest_that_fits() {
        // A whole multiple of 16 in every case, never zero.
        assert_eq!(tex_side(20.0, 16), 320);
        assert_eq!(tex_side(12.0, 16), 192);
        assert_eq!(tex_side(11.314, 16), 176, "181 px rounds to 176");
        assert_eq!(tex_side(0.5, 4), 16, "never below one texel row");
        assert_eq!(tex_side(1.0, 4), 16);

        // Each panel gets its own picture: the atlas counts distinct pictures,
        // so panels sharing one would rightly cost a single entry and this is
        // measuring the budget, not the sharing.
        let panel = |i: usize| Panel {
            name: format!("p{i}"),
            w: 32.0,
            h: 32.0,
            tex: RgbImage::from_pixel(1, 1, Rgb([(i % 251) as u8, (i / 251) as u8, 7])),
        };
        // 250 full panels: 65.5 M px at 16, and the budget holds 12.
        // The ladder steps by one, so it lands on the largest px that fits
        // rather than on the next power of two below it.
        let many: Vec<Panel> = (0..250).map(panel).collect();
        assert_eq!(fit_px_of(&many, 16), 12);
        assert_eq!(fit_px_of(&many, 32), 12, "the same answer from higher up");
        assert!(atlas_area(&many, &pixel_hashes(&many), 12) <= atlas_budget());
        assert!(
            atlas_area(&many, &pixel_hashes(&many), 13) > atlas_budget(),
            "12 must be the largest that fits, not merely one that does"
        );
        assert_eq!(fit_px_of(&many[..2], 16), 16);
        assert_eq!(fit_px_of(&many[..2], 32), 32);

        // The same picture on 250 walls is one atlas entry, so the budget that
        // could not hold them apart holds them together. This is what a city
        // does: one facade hung on hundreds of identical walls.
        let same: Vec<Panel> = (0..250)
            .map(|i| Panel {
                name: format!("s{i}"),
                w: 32.0,
                h: 32.0,
                tex: RgbImage::from_pixel(1, 1, Rgb([9, 9, 9])),
            })
            .collect();
        assert_eq!(atlas_area_of(&same, 16), atlas_area_of(&same[..1], 16));
        assert_eq!(fit_px_of(&same, 16), 16);
        // Never below 4, even when that still does not fit.
        let huge: Vec<Panel> = (0..20_000).map(panel).collect();
        assert_eq!(fit_px_of(&huge, 16), 4);
    }

    #[test]
    fn the_resource_pack_carries_three_files_per_panel_and_no_data_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(tmp.path());
        let panels = vec![Panel {
            name: "f1_0_0_0".to_string(),
            w: 3.0,
            h: 2.0,
            tex: RgbImage::from_pixel(24, 16, Rgb([10, 20, 30])),
        }];
        let report = write_packs_for(&world, panels, 16).unwrap();
        assert_eq!((report.panels, report.px), (1, 16));
        // Display panels are resource pack only: no variants, no level.dat.
        assert!(!world.join("datapacks").exists());

        for rel in ["resources.zip", "resourcepacks/resources.zip"] {
            let file = std::fs::File::open(world.join(rel)).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let read = |archive: &mut zip::ZipArchive<std::fs::File>, name: &str| {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut text)
                    .unwrap();
                text
            };
            let mcmeta: serde_json::Value =
                serde_json::from_str(&read(&mut archive, "pack.mcmeta")).unwrap();
            assert_eq!(mcmeta["pack"]["pack_format"], 46, "{rel}");
            assert_eq!(mcmeta["pack"]["min_format"], 46, "{rel}");
            assert_eq!(mcmeta["pack"]["description"], "Arnis facade panels");

            let def: serde_json::Value =
                serde_json::from_str(&read(&mut archive, "assets/arnis/items/f1_0_0_0.json"))
                    .unwrap();
            assert_eq!(def["model"]["model"], "arnis:item/f1_0_0_0");
            let model: serde_json::Value = serde_json::from_str(&read(
                &mut archive,
                "assets/arnis/models/item/f1_0_0_0.json",
            ))
            .unwrap();
            assert_eq!(model["textures"]["0"], "arnis:block/f1_0_0_0");

            let mut png = Vec::new();
            std::io::Read::read_to_end(
                &mut archive
                    .by_name("assets/arnis/textures/block/f1_0_0_0.png")
                    .unwrap(),
                &mut png,
            )
            .unwrap();
            let img = image::load_from_memory(&png).unwrap();
            assert_eq!((img.width(), img.height()), (48, 32), "{rel}");
        }
    }

    /// A `cols` m wide, `rows` m high wall texture at the lab's 8 px per
    /// metre: red on the node A half, blue on the node B half.
    fn two_tone_tex(cols: u32, rows: u32) -> RgbaImage {
        let width = cols * 8;
        RgbaImage::from_fn(width, rows * 8, |x, _| {
            if x < width / 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        })
    }

    /// A straight wall of `cols` cells along x at `z`, textured from node A
    /// (west) to node B (east), with its outward normal `(0, nz)`.
    fn wall_along_x(
        cells: &mut FnvHashMap<(i32, i32), CellRef>,
        wall: u32,
        x0: i32,
        z: i32,
        cols: i32,
        nz: i8,
    ) {
        for i in 0..cols {
            cells.insert(
                (x0 + i, z),
                CellRef {
                    wall,
                    col: i as u16,
                    nx: 0,
                    nz,
                },
            );
        }
    }

    fn doubles(entity: &HashMap<String, Value>, key: &str) -> Vec<f64> {
        match entity.get(key) {
            Some(Value::List(v)) => v
                .iter()
                .map(|x| match x {
                    Value::Double(d) => *d,
                    other => panic!("{key}: {other:?} is not a double"),
                })
                .collect(),
            other => panic!("{key}: {other:?}"),
        }
    }

    fn floats(compound: &HashMap<String, Value>, key: &str) -> Vec<f64> {
        match compound.get(key) {
            Some(Value::List(v)) => v
                .iter()
                .map(|x| match x {
                    Value::Float(f) => f64::from(*f),
                    other => panic!("{key}: {other:?} is not a float"),
                })
                .collect(),
            other => panic!("{key}: {other:?}"),
        }
    }

    fn transformation(entity: &HashMap<String, Value>) -> &HashMap<String, Value> {
        match entity.get("transformation") {
            Some(Value::Compound(c)) => c,
            other => panic!("transformation: {other:?}"),
        }
    }

    /// The `minecraft:item_model` id the display's item carries.
    fn item_model(entity: &HashMap<String, Value>) -> String {
        let Some(Value::Compound(item)) = entity.get("item") else {
            panic!("no item");
        };
        assert_eq!(item.get("id"), Some(&Value::String(ITEM.to_string())));
        assert_eq!(item.get("count"), Some(&Value::Int(1)));
        let Some(Value::Compound(components)) = item.get("components") else {
            panic!("no components");
        };
        match components.get("minecraft:item_model") {
            Some(Value::String(s)) => s.clone(),
            other => panic!("item_model: {other:?}"),
        }
    }

    /// Panel sizes in the registry, by name.
    fn panel_sizes() -> Vec<(String, f64, f64)> {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<_> = r
            .panels
            .iter()
            .map(|p| (p.name.clone(), p.w, p.h))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Corner pixels of a registered panel's crop: top-left and bottom-right.
    fn panel_corners(name: &str) -> ((u32, u32), [u8; 3], [u8; 3]) {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let tex = &r.panels.iter().find(|p| p.name == name).unwrap().tex;
        let (w, h) = (tex.width(), tex.height());
        ((w, h), tex.get_pixel(0, 0).0, tex.get_pixel(w - 1, h - 1).0)
    }

    #[test]
    fn a_wall_standing_on_fill_starts_at_its_first_wall_block() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 5353;
        // The same 20 by 12 wall, but the building's floor sits two blocks
        // above the terrain, which is what a slope does on the downhill side.
        let walls = vec![FacadeWall::for_test(
            WAY,
            1,
            2,
            20,
            12,
            two_tone_tex(20, 12),
        )];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 100, 50, 20, 1);
        facades::install_displays_for_test(walls, cells, vec![(19, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 2, 0, 12), 1);
        finalize(&mut editor).unwrap();

        let entities = editor.item_displays();
        assert_eq!(entities.len(), 1);
        let e = &entities[0];
        // The first wall block is at y = 3 and the ground two blocks below it.
        // The panel is the photograph's own twelve blocks and starts at the
        // wall block: the two blocks of fill below stay bare rather than
        // carrying pixels the photograph does not have.
        let t = transformation(e);
        assert_eq!(floats(t, "scale"), vec![20.0, 12.0, 1.0]);
        let pos = doubles(e, "Pos");
        assert!(close(pos[1], 9.0), "centre of a 12 block panel based at 3");
        assert_eq!(panel_sizes(), vec![("f5353_0_0_0".to_string(), 20.0, 12.0)]);

        // Nothing was added to the crop: 8 px per metre over 20 by 12 metres.
        let ((w, h), top_left, bottom_right) = panel_corners("f5353_0_0_0");
        assert_eq!((w, h), (160, 96), "the photograph and nothing else");
        assert_eq!(top_left, [255, 0, 0]);
        assert_eq!(bottom_right, [0, 0, 255]);
    }

    #[test]
    fn a_straight_wall_hangs_one_display_covering_all_of_it() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 5252;
        // One 20 block wall at z = 50 facing south, twelve rows high,
        // textured from node A (west) to node B (east).
        let walls = vec![FacadeWall::for_test(
            WAY,
            1,
            2,
            20,
            12,
            two_tone_tex(20, 12),
        )];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 100, 50, 20, 1);
        facades::install_displays_for_test(walls, cells, vec![(19, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        // No wall blocks are placed on purpose: a display needs nothing solid
        // behind it and nothing clear in front, only ground below the top.

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 12), 1);
        // A second tile reaching the same building claims nothing.
        assert_eq!(collect(&mut editor, WAY, 0, 0, 12), 0);
        assert!(editor.item_displays().is_empty(), "nothing hangs yet");

        let report = finalize(&mut editor).unwrap();
        assert_eq!(
            report.stats,
            PlacementStats {
                candidates: 1,
                displays: 1,
                placed: 1,
                dropped: 0,
            }
        );
        let entities = editor.item_displays();
        assert_eq!(entities.len(), 1, "one entity for the whole wall");
        let e = &entities[0];
        assert_eq!(item_model(e), "arnis:f5252_0_0_0");
        assert_eq!(
            e.get("item_display"),
            Some(&Value::String("fixed".to_string()))
        );
        assert_eq!(
            e.get("billboard"),
            Some(&Value::String("fixed".to_string()))
        );
        assert_eq!(e.get("view_range"), Some(&Value::Float(VIEW_RANGE)));
        // Zero, which turns culling off; a box around the entity point would
        // cut a panel this much bigger than itself.
        assert_eq!(e.get("width"), Some(&Value::Float(0.0)));
        assert_eq!(e.get("height"), Some(&Value::Float(0.0)));

        // Centred on the wall, half a block plus PUSH_OUT south of it, and
        // half its height above the first wall block at y = 1.
        let pos = doubles(e, "Pos");
        assert!(close(pos[0], 110.0) && close(pos[1], 7.0) && close(pos[2], 51.0 + PUSH_OUT));
        let t = transformation(e);
        assert_eq!(floats(t, "scale"), vec![20.0, 12.0, 1.0]);
        assert_eq!(floats(t, "translation"), vec![0.0, 0.0, 0.0]);
        assert_eq!(floats(t, "left_rotation"), vec![0.0, 0.0, 0.0, 1.0]);
        assert_eq!(floats(t, "right_rotation"), vec![0.0, 0.0, 0.0, 1.0]);

        assert_eq!(panel_sizes(), vec![("f5252_0_0_0".to_string(), 20.0, 12.0)]);
        // Seen from the south, node A (red) is on the viewer's left.
        assert_eq!(
            panel_corners("f5252_0_0_0"),
            ((160, 96), [255, 0, 0], [0, 0, 255])
        );

        // Settling again finds nothing pending.
        assert!(finalize(&mut editor).is_some());
        assert_eq!(editor.item_displays().len(), 1);
        assert_eq!(stats().displays, 1);
    }

    #[test]
    fn a_diagonal_wall_gets_one_flat_quad_instead_of_a_staircase() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 5353;
        // A wall at 45 degrees from node A at (100, 50) to node B at (107, 57):
        // eight Bresenham cells, one texture column each. Hung on the block
        // grid this would be a staircase of nine axis-aligned panels, one per
        // open axis face; here it is a single quad along the true line.
        let walls = vec![FacadeWall::for_test(WAY, 11, 12, 8, 6, two_tone_tex(8, 6))];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        for i in 0..8 {
            cells.insert(
                (100 + i, 50 + i),
                CellRef {
                    wall: 0,
                    col: i as u16,
                    nx: 1,
                    nz: 0,
                },
            );
        }
        facades::install_displays_for_test(walls, cells, vec![(7, 7)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 1);
        let report = finalize(&mut editor).unwrap();
        assert_eq!(report.stats.displays, 1, "one quad, not nine panels");

        let entities = editor.item_displays();
        assert_eq!(entities.len(), 1);
        let e = &entities[0];
        let pos = doubles(e, "Pos");
        let out = ROOT_HALF + PUSH_OUT;
        assert!(close(pos[0], 104.0 + out * ROOT_HALF), "x = {}", pos[0]);
        assert!(close(pos[1], 4.0));
        assert!(close(pos[2], 54.0 - out * ROOT_HALF), "z = {}", pos[2]);
        let t = transformation(e);
        // The quad spans the eight cell centres plus half a step at each end.
        let width = 98.0f64.sqrt() + std::f64::consts::SQRT_2;
        assert!(
            close(floats(t, "scale")[0], width),
            "{:?}",
            floats(t, "scale")
        );
        assert_eq!(floats(t, "scale")[1], 6.0);
        // 135 degrees about +y, so the quad's front points north-east.
        let rot = floats(t, "left_rotation");
        assert!(
            close(rot[1], 0.923_879_5) && close(rot[3], 0.382_683_4),
            "{rot:?}"
        );

        // The crop is mirrored: from the north-east, node A (red) is on the
        // viewer's right.
        assert_eq!(
            panel_corners("f5353_0_0_0"),
            ((64, 48), [0, 0, 255], [255, 0, 0])
        );
    }

    #[test]
    fn a_long_wall_is_cut_and_a_buried_one_is_dropped() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 5454;
        // 70 blocks along x at z = 50 facing south, 40 rows high: three
        // pieces along the wall and two up it.
        let walls = vec![FacadeWall::for_test(
            WAY,
            1,
            2,
            70,
            40,
            two_tone_tex(70, 40),
        )];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 100, 50, 70, 1);
        facades::install_displays_for_test(walls, cells, vec![(69, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(300.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 40), 1);
        let report = finalize(&mut editor).unwrap();
        assert_eq!(report.stats.displays, 6, "3 along by 2 up");
        let mut names: Vec<(String, f64, f64)> = panel_sizes();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            names,
            vec![
                ("f5454_0_0_0".to_string(), 32.0, 32.0),
                ("f5454_0_0_32".to_string(), 32.0, 8.0),
                ("f5454_0_32_0".to_string(), 32.0, 32.0),
                ("f5454_0_32_32".to_string(), 32.0, 8.0),
                ("f5454_0_64_0".to_string(), 6.0, 32.0),
                ("f5454_0_64_32".to_string(), 6.0, 8.0),
            ]
        );

        // A wall whose whole height sits under the terrain shows nothing.
        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, -60, 0, 40), 1);
        let report = finalize(&mut editor).unwrap();
        assert_eq!(
            report.stats,
            PlacementStats {
                candidates: 1,
                displays: 0,
                placed: 0,
                dropped: 1,
            }
        );
        assert_eq!(editor.item_displays().len(), 6, "no new entity");
    }

    #[test]
    fn flush_region_settles_only_the_candidates_touching_that_region() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 5555;
        // One south-facing wall straddling the region border at x = 512, and
        // one well inside region (0, 0).
        let walls = vec![
            FacadeWall::for_test(WAY, 31, 32, 10, 6, two_tone_tex(10, 6)),
            FacadeWall::for_test(WAY, 33, 34, 4, 6, two_tone_tex(4, 6)),
        ];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 507, 50, 10, 1);
        wall_along_x(&mut cells, 1, 100, 50, 4, 1);
        facades::install_displays_for_test(walls, cells, vec![(9, 0), (3, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(600.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 2);
        // Region (1, 0) holds the east end of the long wall and nothing else.
        flush_region(&mut editor, 1, 0);
        assert_eq!(editor.item_displays().len(), 1);
        // Flushing it again, or a region with no candidates, changes nothing.
        flush_region(&mut editor, 1, 0);
        flush_region(&mut editor, 5, 5);
        assert_eq!(editor.item_displays().len(), 1);
        // The rest waits for the end.
        let report = finalize(&mut editor).unwrap();
        assert_eq!(report.stats.displays, 2);
        assert_eq!(editor.item_displays().len(), 2);
    }

    #[test]
    fn a_wall_merged_over_five_ring_edges_hangs_as_one_run_of_quads() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        // r147094's south wall as one exported wall of 65 columns over its
        // five OSM edges (see `facades::test_fixtures`), textured with one
        // colour per metre column so a crop tells which columns it holds.
        // Cut at `MAX_PANEL` that is three pieces: 32, 32 and 1.
        use crate::element_processing::buildings::relation_ring_id;
        use crate::osm_parser::ProcessedElement;
        let relation = facades::test_fixtures::r147094_relation();
        let ring_id = relation_ring_id(147094, 0);
        let column_colour = |m: u32| {
            let m = m as u8;
            [3 * m, 255 - 3 * m, 100]
        };
        let tex = RgbaImage::from_fn(65 * 8, 48, |x, _| {
            let [r, g, b] = column_colour(x / 8);
            image::Rgba([r, g, b, 255])
        });
        let edges = [
            (21486944, 2545319959, 0.0, 6.0),
            (2545319959, 2545319965, 6.0, 30.0),
            (2545319965, 1121737122, 30.0, 39.0),
            (1121737122, 2545319977, 39.0, 53.0),
            (2545319977, 410874364, 53.0, 65.0),
        ];
        let walls = vec![FacadeWall::for_test_relation_edges(
            147094,
            &edges,
            0.0,
            65,
            6,
            Some(tex),
        )];
        let elements = vec![ProcessedElement::Relation(relation)];
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        facades::install_display_elements_for_test(walls, &elements, &xzbbox, 1.0);

        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);

        reset(true, 16);
        assert_eq!(collect(&mut editor, ring_id, 0, 0, 6), 1);
        let report = finalize(&mut editor).unwrap();
        assert_eq!(report.stats.displays, 3);

        // Each piece holds exactly the metre columns of its blocks, left to
        // right along the ring from node 21486944, and hangs centred on them
        // half a block plus PUSH_OUT south of the wall.
        let mut placed: Vec<(f64, f64, f64)> = editor
            .item_displays()
            .iter()
            .map(|e| {
                let pos = doubles(e, "Pos");
                (pos[0], pos[2], floats(transformation(e), "scale")[0])
            })
            .collect();
        placed.sort_by(|a, b| a.0.total_cmp(&b.0));
        for ((x, z, w), (want_x, want_w)) in
            placed.iter().zip([(36.0, 32.0), (68.0, 32.0), (84.5, 1.0)])
        {
            assert!(close(*x, want_x) && close(*w, want_w), "{x} {w}");
            assert!(close(*z, 61.0 + PUSH_OUT));
        }
        for (name, first_col, w) in [
            ("f{id}_0_0_0", 0u32, 32u32),
            ("f{id}_0_32_0", 32, 32),
            ("f{id}_0_64_0", 64, 1),
        ] {
            let name = name.replace("{id}", &ring_id.to_string());
            let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            let crop = &r.panels.iter().find(|p| p.name == name).unwrap().tex;
            assert_eq!((crop.width(), crop.height()), (w * 8, 48), "{name}");
            for k in 0..w {
                assert_eq!(
                    crop.get_pixel(k * 8 + 4, 24).0,
                    column_colour(first_col + k),
                    "{name}, block {k}"
                );
            }
        }
    }

    #[test]
    fn only_the_photos_mode_hangs_displays() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        use crate::args::FacadeMode;
        assert!(!FacadeMode::Blocks.places_displays());
        assert!(FacadeMode::Photos.places_displays());

        const WAY: u64 = 5656;
        let make = || {
            let walls = vec![FacadeWall::for_test(WAY, 1, 2, 4, 6, two_tone_tex(4, 6))];
            let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
            wall_along_x(&mut cells, 0, 100, 50, 4, 1);
            (walls, cells)
        };
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        for i in 0..4 {
            for y in 1..=6 {
                editor.set_block_absolute(
                    crate::block_definitions::SMOOTH_STONE,
                    100 + i,
                    y,
                    50,
                    None,
                    None,
                );
            }
        }

        // Blocks selected: the store carries the textured wall, but nothing is
        // collected however willing the registry is.
        let (walls, cells) = make();
        facades::install_blocks_for_test(walls, cells, vec![(3, 0)], 1.0);
        assert!(!facades::displays_enabled());
        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 0);

        // Photos selected: the same wall is recorded.
        let (walls, cells) = make();
        facades::install_displays_for_test(walls, cells, vec![(3, 0)], 1.0);
        assert!(facades::displays_enabled());
        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 1);
    }

    /// Every tag of one finished display, key by key and type by type, against
    /// what a 1.21.4+ client reads. The shape is easy to break by accident and
    /// the game says nothing when it is wrong: an item stack it cannot decode
    /// becomes an empty one and the entity then draws nothing at all.
    #[test]
    fn one_display_carries_exactly_the_tags_the_client_reads() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 5757;
        let walls = vec![FacadeWall::for_test(WAY, 1, 2, 8, 5, two_tone_tex(8, 5))];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 100, 50, 8, 1);
        facades::install_displays_for_test(walls, cells, vec![(7, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 5), 1);
        finalize(&mut editor).unwrap();
        let entities = editor.item_displays();
        assert_eq!(entities.len(), 1);
        let e = &entities[0];

        // Nothing beyond this set, so a stray tag cannot creep in unnoticed.
        let mut keys: Vec<&str> = e.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "Air",
                "FallDistance",
                "Fire",
                "Motion",
                "OnGround",
                "PortalCooldown",
                "Pos",
                "Rotation",
                "UUID",
                "billboard",
                "height",
                "id",
                "item",
                "item_display",
                "transformation",
                "view_range",
                "width",
            ]
        );

        // `item_display` is the entity's own transform name, not the model's.
        assert_eq!(
            e.get("id"),
            Some(&Value::String("minecraft:item_display".to_string())),
            "the entity id, and the only one whose `item` tag takes a stack"
        );
        assert_eq!(
            e.get("item_display"),
            Some(&Value::String("fixed".to_string()))
        );
        assert_eq!(
            e.get("billboard"),
            Some(&Value::String("fixed".to_string()))
        );
        assert_eq!(e.get("view_range"), Some(&Value::Float(4.0)));
        assert_eq!(e.get("width"), Some(&Value::Float(0.0)));
        assert_eq!(e.get("height"), Some(&Value::Float(0.0)));
        // A `fixed` billboard still turns the model by the entity's own yaw
        // and pitch, so those must stay at zero or they compound with the
        // quaternion below.
        assert_eq!(floats(e, "Rotation"), vec![0.0, 0.0]);
        // Three doubles, the exact point the quad is centred on.
        assert_eq!(doubles(e, "Pos").len(), 3);

        // The 1.20.5+ item stack: lowercase `count`, components by full id,
        // `minecraft:item_model` a bare namespaced string. A block item, so
        // the client draws the panel in the opaque entity pass.
        let Some(Value::Compound(item)) = e.get("item") else {
            panic!("no item compound");
        };
        let mut item_keys: Vec<&str> = item.keys().map(String::as_str).collect();
        item_keys.sort_unstable();
        assert_eq!(item_keys, ["components", "count", "id"]);
        assert_eq!(item.get("id"), Some(&Value::String(ITEM.to_string())));
        assert!(
            ITEM == "minecraft:stone",
            "the carrier must stay a block item, see ITEM"
        );
        assert_eq!(item.get("count"), Some(&Value::Int(1)));
        let Some(Value::Compound(components)) = item.get("components") else {
            panic!("no components compound");
        };
        assert_eq!(
            components.keys().collect::<Vec<_>>(),
            vec!["minecraft:item_model"]
        );
        assert_eq!(
            components.get("minecraft:item_model"),
            Some(&Value::String("arnis:f5757_0_0_0".to_string())),
            "the id the pack's assets/arnis/items/<name>.json answers to"
        );

        // The transformation: four fields, floats throughout, quaternions as
        // [x, y, z, w]. The scale is the panel's size in blocks because the
        // model is one block wide and the game applies it before the rotation.
        let t = transformation(e);
        let mut t_keys: Vec<&str> = t.keys().map(String::as_str).collect();
        t_keys.sort_unstable();
        assert_eq!(
            t_keys,
            ["left_rotation", "right_rotation", "scale", "translation"]
        );
        assert_eq!(floats(t, "scale"), vec![8.0, 5.0, 1.0]);
        assert_eq!(floats(t, "translation"), vec![0.0, 0.0, 0.0]);
        assert_eq!(floats(t, "right_rotation"), vec![0.0, 0.0, 0.0, 1.0]);
        let left = floats(t, "left_rotation");
        assert_eq!(left.len(), 4);
        let norm: f64 = left.iter().map(|v| v * v).sum::<f64>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-6,
            "left_rotation must be a unit quaternion, got {left:?}"
        );

        assert_eq!(panel_sizes(), vec![("f5757_0_0_0".to_string(), 8.0, 5.0)]);
    }

    /// The pack's exact layout: which files a panel produces, where they sit
    /// and what each one has to say for the chain from the entity's
    /// `minecraft:item_model` id down to a stitched sprite to close.
    #[test]
    fn the_pack_lays_a_panel_out_the_way_the_client_resolves_it() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(tmp.path());
        let panels = vec![Panel {
            name: "f7_1_0_0".to_string(),
            w: 4.0,
            h: 3.0,
            tex: RgbImage::from_pixel(32, 24, Rgb([7, 8, 9])),
        }];
        write_packs_for(&world, panels, 16).unwrap();
        // Staged beside its final name and renamed into place, nothing left.
        assert!(!world.join("resources.zip.tmp").exists());
        assert!(!world.join("resourcepacks/resources.zip.tmp").exists());

        let file = std::fs::File::open(world.join("resources.zip")).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut names: Vec<String> = archive.file_names().map(str::to_string).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                // The marker that tells our pack from a user's own.
                PACK_MARKER.to_string(),
                "assets/arnis/items/f7_1_0_0.json".to_string(),
                "assets/arnis/models/item/f7_1_0_0.json".to_string(),
                "assets/arnis/textures/block/f7_1_0_0.png".to_string(),
                "pack.mcmeta".to_string(),
            ],
            "one item definition, one model and one texture per panel, nothing else"
        );

        let read = |archive: &mut zip::ZipArchive<std::fs::File>, name: &str| {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut text).unwrap();
            serde_json::from_str::<serde_json::Value>(&text).unwrap()
        };

        // The pack has to declare a floor of 46, the first format that
        // resolves an item's model through `assets/<ns>/items/`, and no
        // ceiling that would shut a later game out. 1.21.11 reads 75.
        let mcmeta = read(&mut archive, "pack.mcmeta");
        assert_eq!(mcmeta["pack"]["min_format"], 46);
        assert_eq!(mcmeta["pack"]["pack_format"], 46);
        assert_eq!(mcmeta["pack"]["max_format"], serde_json::json!([999, 0]));

        // items/<name>.json is what the entity's component names; it points at
        // models/item/<name>.json, which points at textures/block/<name>.png.
        let def = read(&mut archive, "assets/arnis/items/f7_1_0_0.json");
        assert_eq!(
            def,
            serde_json::json!({
                "model": { "type": "minecraft:model", "model": "arnis:item/f7_1_0_0" }
            })
        );
        let model = read(&mut archive, "assets/arnis/models/item/f7_1_0_0.json");
        assert_eq!(
            model,
            serde_json::json!({
                "textures": {
                    "0": "arnis:block/f7_1_0_0",
                    "particle": "arnis:block/f7_1_0_0"
                },
                "elements": [{
                    "from": [0.0, 0.0, 7.9],
                    "to": [16.0, 16.0, 8.1],
                    "faces": {
                        "north": { "uv": [0, 0, 16, 16], "texture": "#0" },
                        "south": { "uv": [0, 0, 16, 16], "texture": "#0" }
                    }
                }],
                "display": {
                    "fixed": {
                        "rotation": [0, 0, 0],
                        "translation": [0, 0, 0],
                        "scale": [1, 1, 1]
                    }
                }
            }),
            "every texture of an item model has to come from one atlas, and \
             textures/block puts them all in the block one"
        );

        // A whole multiple of 16 on both sides or the block atlas loses a
        // mipmap level for every block in the world.
        let mut png = Vec::new();
        std::io::Read::read_to_end(
            &mut archive
                .by_name("assets/arnis/textures/block/f7_1_0_0.png")
                .unwrap(),
            &mut png,
        )
        .unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!((img.width(), img.height()), (64, 48));
        assert_eq!((img.width() % 16, img.height() % 16), (0, 0));

        // Resource pack only, and the same bytes at both places a world pack
        // has ever been read from.
        assert!(!world.join("datapacks").exists());
        assert_eq!(
            std::fs::read(world.join("resources.zip")).unwrap(),
            std::fs::read(world.join("resourcepacks/resources.zip")).unwrap()
        );
    }

    /// A world the user has dressed themselves keeps its pack: ours goes in,
    /// theirs is kept beside it rather than overwritten.
    #[test]
    fn a_resource_pack_arnis_did_not_write_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.zip");

        // A pack that is not ours: a real zip whose pack.mcmeta says so.
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::FileOptions::default();
        zip.start_file("pack.mcmeta", opts).unwrap();
        zip.write_all(br#"{"pack":{"description":"Made with Arnis, then hand painted"}}"#)
            .unwrap();
        let theirs = zip.finish().unwrap().into_inner();
        std::fs::write(&path, &theirs).unwrap();
        assert!(!is_arnis_pack(&path));

        write_world_pack(&path, b"ours").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"ours");
        assert_eq!(
            std::fs::read(dir.path().join("resources.zip.bak")).unwrap(),
            theirs,
            "the user's pack was not kept"
        );

        // A second generation replaces our own pack in place and does not
        // bury the rescued one.
        std::fs::write(&path, b"ours-v1").unwrap();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("pack.mcmeta", opts).unwrap();
        zip.write_all(br#"{"pack":{"description":"Arnis facade panels"}}"#)
            .unwrap();
        zip.start_file(PACK_MARKER, opts).unwrap();
        zip.write_all(b"arnis").unwrap();
        std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();
        write_world_pack(&path, b"ours-v2").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"ours-v2");
        assert!(!dir.path().join("resources.zip.bak1").exists());
    }

    #[test]
    fn walls_cut_at_max_panel() {
        assert_eq!(cut(33, MAX_PANEL), vec![(0, 32), (32, 33)]);
        assert_eq!(cut(32, MAX_PANEL), vec![(0, 32)]);
        assert_eq!(cut(65, MAX_PANEL), vec![(0, 32), (32, 64), (64, 65)]);
        assert!(cut(0, MAX_PANEL).is_empty());
    }

    #[test]
    fn pack_mcmeta_has_the_shape_every_1_21_accepts() {
        let v: serde_json::Value = serde_json::from_str(&pack_mcmeta(46)).unwrap();
        assert_eq!(v["pack"]["pack_format"], 46);
        assert_eq!(v["pack"]["supported_formats"], serde_json::json!([46, 999]));
        assert_eq!(v["pack"]["min_format"], 46);
        assert_eq!(v["pack"]["max_format"], serde_json::json!([999, 0]));
        assert!(v["pack"]["description"].is_string());
        assert!(v.get("overlays").is_none());
    }

    #[test]
    fn missing_pixels_take_the_wall_colour_of_their_row_and_thin_panels_are_skipped() {
        // A 6 x 9 crop: three rows of colour A, three of colour B with one
        // hole, three rows without any data.
        let a = image::Rgba([10, 20, 30, 255]);
        let b = image::Rgba([200, 210, 220, 255]);
        let none = image::Rgba([1, 2, 3, 0]);
        let img = RgbaImage::from_fn(6, 9, |x, y| match y {
            0..=2 => a,
            3..=5 if (x, y) != (2, 4) => b,
            _ => none,
        });
        let out = fill_missing(&img, Some((90, 90, 90))).unwrap();
        // The photograph is untouched.
        assert_eq!(out.get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(out.get_pixel(5, 5).0, [200, 210, 220]);
        // The hole takes its row's colour, not a smear of the rows above.
        assert_eq!(out.get_pixel(2, 4).0, [200, 210, 220]);
        // Rows without data take the crop's median, A (18 pixels against
        // 17). The first of them still sees row 5 through the blur; the
        // others are flat.
        assert!(out.get_pixel(3, 6).0[0] > 10);
        assert_eq!(out.get_pixel(3, 7).0, [10, 20, 30]);
        assert_eq!(out.get_pixel(3, 8).0, [10, 20, 30]);
        // Fewer than a quarter valid: no panel. A quarter: a panel.
        let mut thin = RgbaImage::from_pixel(4, 4, none);
        for x in 0..3 {
            thin.put_pixel(x, 0, a);
        }
        assert!(fill_missing(&thin, None).is_none());
        thin.put_pixel(3, 0, a);
        assert!(fill_missing(&thin, None).is_some());
        // Nothing valid at all: no panel, whatever the fallback.
        let empty = RgbaImage::from_pixel(4, 4, none);
        assert!(fill_missing(&empty, Some((1, 2, 3))).is_none());
    }
}
