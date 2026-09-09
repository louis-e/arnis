//! Facade textures as custom painting variants (Java 1.21+).
//!
//! Hanging a texture as map tiles in item frames would cost one dynamic map
//! texture and one entity per block, so a wall of 20 x 12 blocks would be 240
//! of each. A painting covers up to 16 x 16 blocks with one entity, and every
//! painting in the world renders from one atlas, so the same wall is two
//! entities and no maps at all.
//!
//! Three steps:
//! * `collect` runs when a building's wall ring has been built. It walks the
//!   building's textured wall cells, finds the outward faces that are open,
//!   groups them into flat runs and records every run as a panel candidate in
//!   a process-wide registry, reserving its air cells so decals stay out.
//! * `finalize` runs once every block is final, before the world is saved
//!   (under stream-to-disk eviction `flush_region` does the same for each
//!   region right before it leaves memory). It checks every candidate against
//!   the finished world, keeps the largest parts that still hold, cuts them
//!   into panels of at most 16 x 16 blocks, hangs one painting entity per
//!   panel and crops the panel's texture out of the 8 px/m wall texture.
//! * `write_packs` runs once the world is saved. It writes the variants as a
//!   data pack (`<world>/datapacks/arnis_facades`, enabled in level.dat) and
//!   the textures as the world resource pack (`<world>/resources.zip`, and the
//!   same file at `<world>/resourcepacks/resources.zip`, where 26.1 looks).
//!
//! The game re-checks every painting every 100 ticks and drops it as an item
//! unless every block behind it is solid, nothing with a collision box sits in
//! its air cells and no other hanging entity overlaps it. Hanging at the end
//! is what makes the first two hold: whatever went up after the wall (trees,
//! street furniture, signs, a sidewalk on a slope, a roof edge that swapped
//! the top row for slabs) is visible to the check. The third holds because no
//! two candidates ever share an air cell, and the decal code refuses to hang a
//! frame in a reserved one.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::Mutex;

use fnv::{FnvHashMap, FnvHashSet};
use image::{RgbImage, RgbaImage};

use super::facades::{self, FacadeStore};
use crate::block_definitions::{
    Block, AIR, BLUE_STAINED_GLASS, BLUE_STAINED_GLASS_PANE, GLASS, GLASS_PANE, GRAY_STAINED_GLASS,
    GRAY_STAINED_GLASS_PANE,
};
use crate::colors::RGBTuple;
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};
use crate::world_editor::WorldEditor;

/// Largest painting edge the game allows, in blocks.
pub const MAX_PANEL: i32 = 16;

/// Pixels per metre of the lab's `_tex.png` wall textures.
pub(super) const TEX_PX_PER_M: f64 = 8.0;

/// Panels whose crop carries fewer valid pixels than this stay plain blocks.
const MIN_VALID_FRACTION: f64 = 0.25;

/// The paintings atlas has to fit the smallest common GPU texture limit,
/// 8192 x 8192, with room for the vanilla paintings and stitching padding.
/// The atlas side the panels are budgeted against, in pixels.
///
/// Minecraft stitches every block texture into one image and grows it in powers
/// of two up to the largest texture the driver actually accepts. Overflowing it
/// is not a soft failure: the game drops the whole pack, switches off the
/// player's other resource packs with it, and saves that to options.txt.
///
/// 8192 is the safe floor. The game's own stated minimum is an OpenGL 4.4 GPU,
/// and that specification requires at least 16384, so `ATLAS_SIDE_HIGH` is
/// defensible; it is not the default because 16384 x 8192 is 716 MB of atlas
/// VRAM against a stated 2 GB minimum, which is the player's call and not ours.
pub const ATLAS_SIDE_STANDARD: u32 = 8192;
pub const ATLAS_SIDE_HIGH: u32 = 16384;

/// Usable pixels in an atlas of `side`, after packing waste.
///
/// The six tenths is a packing allowance, not room for vanilla: every vanilla
/// block sprite together is 0.44 Mpx, which is under one per cent of an 8192
/// atlas. Measured packing efficiency is 84 to 95 per cent, so this is
/// conservative by a third, deliberately: from 1.21.11 the player's
/// anisotropic filtering setting pads every sprite and can add 29 per cent to
/// the same pack, and a pack that stitches here must still stitch there.
pub(super) const fn atlas_budget_for(side: u32) -> u64 {
    (side as u64) * (side as u64) * 6 / 10
}

/// The budget this run is working to, set once from the settings.
static ATLAS_BUDGET: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(atlas_budget_for(ATLAS_SIDE_STANDARD));

/// Chooses the atlas the panels are budgeted against for this generation.
pub fn set_atlas_side(side: u32) {
    ATLAS_BUDGET.store(atlas_budget_for(side), std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn atlas_budget() -> u64 {
    ATLAS_BUDGET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Lowest resolution the budget rule falls back to.
pub(super) const MIN_PX_PER_BLOCK: u32 = 4;

/// Folder of the data pack under `<world>/datapacks`.
pub const DATAPACK_NAME: &str = "arnis_facades";

/// Namespace of the variants and their textures.
const NAMESPACE: &str = "arnis";

/// Data pack format of 1.21 and resource pack format of 1.21; the mcmeta
/// declares support up to format 999 on top, so every later version loads
/// the packs as well.
const DATAPACK_FORMAT: u32 = 48;
const RESOURCEPACK_FORMAT: u32 = 34;

/// Painting `facing` byte for an axis-snapped outward normal in Arnis' frame
/// (x east, z south): 0 south, 1 west, 2 north, 3 east. The front of the
/// painting points along the normal.
pub fn facing_for_normal(nx: i32, nz: i32) -> Option<i8> {
    match (nx.signum(), nz.signum()) {
        (0, 1) => Some(0),
        (-1, 0) => Some(1),
        (0, -1) => Some(2),
        (1, 0) => Some(3),
        _ => None,
    }
}

/// Unit normal out of the wall for a facing byte.
pub fn normal_for_facing(facing: i8) -> (i32, i32) {
    match facing {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    }
}

/// The viewer's right, standing in front of a painting with this facing. The
/// game leans even-sized paintings half a block this way.
pub fn right_vector(facing: i8) -> (i32, i32) {
    match facing {
        // Faces south: the viewer looks north, their right is east.
        0 => (1, 0),
        // Faces west: the viewer looks east, their right is south.
        1 => (0, 1),
        // Faces north: the viewer looks south, their right is west.
        2 => (-1, 0),
        // Faces east: the viewer looks west, their right is north.
        _ => (0, -1),
    }
}

/// The outward axis faces of a wall cell: the face along sign(nx) and the one
/// along sign(nz) of the wall's true normal. That normal is the perpendicular
/// of the node A to node B direction `dir` (in world blocks) that points the
/// way the axis-snapped normal `(snx, snz)` does. A wall on an axis has one
/// face; a diagonal wall is a Bresenham staircase whose steps show both.
pub fn outward_faces(dir: (i32, i32), snx: i32, snz: i32) -> Vec<(i32, i32)> {
    let (dx, dz) = dir;
    // Dot product of the perpendicular (dz, -dx) with the snapped normal.
    let side = dz * snx - dx * snz;
    let (nx, nz) = if side > 0 {
        (dz, -dx)
    } else if side < 0 {
        (-dz, dx)
    } else {
        (snx, snz)
    };
    let mut faces = Vec::with_capacity(2);
    if nx != 0 {
        faces.push((nx.signum(), 0));
    }
    if nz != 0 {
        faces.push((0, nz.signum()));
    }
    faces
}

/// Anchor block of a `w` x `h` painting whose viewer's-left, bottom air cell
/// is `(left_x, bottom_y, left_z)`. Mirrors `Painting.calculateBoundingBox`:
/// the painting spans anchor - floor((w - 1) / 2) to anchor + floor(w / 2)
/// along the viewer's right and likewise upwards, so the anchor is the left
/// cell moved floor((w - 1) / 2) to the right and floor((h - 1) / 2) up.
pub fn anchor(
    left_x: i32,
    bottom_y: i32,
    left_z: i32,
    facing: i8,
    w: i32,
    h: i32,
) -> (i32, i32, i32) {
    let (rx, rz) = right_vector(facing);
    let k = (w - 1) / 2;
    (left_x + rx * k, bottom_y + (h - 1) / 2, left_z + rz * k)
}

/// Air cells a `w` x `h` painting anchored at `(ax, ay, az)` covers, computed
/// the way the game does it.
pub fn covered_cells(
    ax: i32,
    ay: i32,
    az: i32,
    facing: i8,
    w: i32,
    h: i32,
) -> Vec<(i32, i32, i32)> {
    let (rx, rz) = right_vector(facing);
    let mut out = Vec::with_capacity((w * h).max(0) as usize);
    for k in -((w - 1) / 2)..=(w / 2) {
        for r in -((h - 1) / 2)..=(h / 2) {
            out.push((ax + rx * k, ay + r, az + rz * k));
        }
    }
    out
}

/// True when the wall texture has to be mirrored: its columns run from node A
/// to node B, and the painting's left edge must be the viewer's left.
pub fn flip_crop(dir_ab: (i32, i32), facing: i8) -> bool {
    let (rx, rz) = right_vector(facing);
    dir_ab.0 * rx + dir_ab.1 * rz < 0
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

/// A wall column that can take paintings: its cell, its texture column and
/// the wall rows [lo, hi) (counted from the first wall block) it can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    pub bx: i32,
    pub bz: i32,
    pub col: u16,
    pub lo: i32,
    pub hi: i32,
}

/// Splits wall columns into runs one flat painting can span: consecutive
/// along the viewer's right, on one line (the same coordinate along the
/// normal, so a Bresenham staircase breaks at every step) and with the same
/// usable rows. Columns without usable rows are dropped.
pub fn runs(mut columns: Vec<Column>, facing: i8) -> Vec<Vec<Column>> {
    let (rx, rz) = right_vector(facing);
    let (nx, nz) = normal_for_facing(facing);
    let along = |c: &Column| c.bx * rx + c.bz * rz;
    let depth = |c: &Column| c.bx * nx + c.bz * nz;
    columns.sort_by_key(|c| (along(c), depth(c)));

    let mut out: Vec<Vec<Column>> = Vec::new();
    for c in columns {
        if c.lo >= c.hi {
            continue;
        }
        let continues = out.last().and_then(|run| run.last()).is_some_and(|p| {
            along(&c) == along(p) + 1 && depth(&c) == depth(p) && c.lo == p.lo && c.hi == p.hi
        });
        match out.last_mut() {
            Some(run) if continues => run.push(c),
            _ => out.push(vec![c]),
        }
    }
    out
}

/// The longest stretch of rows in 0..n for which `ok` holds, lowest first on
/// ties, as [lo, hi). Empty when none does.
fn longest_true_range(n: i32, ok: impl Fn(i32) -> bool) -> (i32, i32) {
    let mut best = (0, 0);
    let mut start = None;
    for r in 0..=n {
        let good = r < n && ok(r);
        match (good, start) {
            (true, None) => start = Some(r),
            (false, Some(s)) => {
                if r - s > best.1 - best.0 {
                    best = (s, r);
                }
                start = None;
            }
            _ => {}
        }
    }
    best
}

/// The full block a glass pane stands for. Panes read as solid to the game's
/// painting check, but a full block behind the painting is the safe backing.
fn pane_to_full(block: Block) -> Option<Block> {
    if block == GLASS_PANE {
        Some(GLASS)
    } else if block == GRAY_STAINED_GLASS_PANE {
        Some(GRAY_STAINED_GLASS)
    } else if block == BLUE_STAINED_GLASS_PANE {
        Some(BLUE_STAINED_GLASS)
    } else if block.name().ends_with("_pane") {
        Some(GLASS)
    } else {
        None
    }
}

/// True when a painting's air cell may hold this: nothing, or a block without
/// a collision box (the ground cover the game's check ignores). A carpet is
/// not here, its sixteenth of a block does collide.
pub fn is_clear(block: Option<Block>) -> bool {
    let Some(block) = block else {
        return true;
    };
    block == AIR
        || matches!(
            block.name(),
            "cave_air"
                | "short_grass"
                | "grass"
                | "tall_grass"
                | "fern"
                | "large_fern"
                | "dead_bush"
                | "dandelion"
                | "poppy"
                | "blue_orchid"
                | "azure_bluet"
                | "snow"
                | "seagrass"
                | "tall_seagrass"
        )
}

/// True for a block the game accepts as the wall behind a painting: a placed
/// full block. Partial shapes (slabs, stairs, panes, fences, doors, signs and
/// the like), plants and fluids are not; glass is, a glass pane is not.
pub fn is_full_solid(block: Block) -> bool {
    if is_clear(Some(block)) {
        return false;
    }
    const PARTIAL_SUFFIXES: &[&str] = &[
        "_slab",
        "_stairs",
        "_wall",
        "_pane",
        "_fence",
        "_fence_gate",
        "_door",
        "_trapdoor",
        "_sign",
        "_button",
        "_pressure_plate",
        "_carpet",
        "_banner",
        "_torch",
        "_bars",
        "_rail",
        "_rod",
        "_bed",
        "_cauldron",
        "_anvil",
        "_hook",
        "_pot",
        "_sapling",
        "_head",
        "_skull",
        "_plant",
        "_pickle",
    ];
    const PARTIAL: &[&str] = &[
        "water",
        "lava",
        "chain",
        "ladder",
        "vine",
        "lily_pad",
        "rail",
        "scaffolding",
        "cobweb",
        "lantern",
        "soul_lantern",
        "anvil",
        "cauldron",
        "chest",
        "hopper",
        "composter",
        "grindstone",
        "brewing_stand",
        "daylight_detector",
        "bamboo",
        "sugar_cane",
        "kelp",
        "farmland",
        "dirt_path",
        "lever",
        "torch",
        "campfire",
        "bell",
        "lectern",
        "stonecutter",
        "enchanting_table",
        "cake",
        "candle",
        "wheat",
        "carrots",
        "potatoes",
    ];
    let name = block.name();
    !(name.starts_with("potted_")
        || PARTIAL.contains(&name)
        || PARTIAL_SUFFIXES.iter().any(|s| name.ends_with(s)))
}

/// One painting variant: its texture crop at the lab's resolution, resampled
/// to the final size when the packs are written.
pub struct Panel {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub tex: RgbImage,
}

/// Wall cells with their texture columns, by wall index and painting facing.
type FaceGroups = BTreeMap<(u32, i8), Vec<(i32, i32, u16)>>;

/// One run of wall faces recorded while the building went up: the cells one
/// flat painting could span, waiting for the finished world to confirm them.
struct Candidate {
    way_id: u64,
    wall: u32,
    facing: i8,
    /// Absolute y of wall row 0.
    base_y: i32,
    /// Wall rows [lo, hi) the run was collected with.
    lo: i32,
    hi: i32,
    /// Wall cells in viewer's-right order, each with its texture column.
    cells: Vec<(i32, i32, u16)>,
}

/// What became of the candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlacementStats {
    /// Runs collected while the walls went up.
    pub candidates: usize,
    /// Painting entities hung.
    pub paintings: usize,
    /// Candidates hung exactly as collected.
    pub intact: usize,
    /// Candidates that lost cells to later blocks and were hung smaller.
    pub shrunk: usize,
    /// Candidates that produced no painting.
    pub dropped: usize,
}

/// Panels of the world being generated, filled from the tile threads.
struct Registry {
    enabled: bool,
    px: u32,
    panels: Vec<Panel>,
    /// Air cells claimed by a candidate, across every building.
    used: FnvHashSet<(i32, i32, i32)>,
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
    used: FnvHashSet::with_hasher(fnv::FnvBuildHasher::new()),
    candidates: Vec::new(),
    by_region: FnvHashMap::with_hasher(fnv::FnvBuildHasher::new()),
    stats: PlacementStats {
        candidates: 0,
        paintings: 0,
        intact: 0,
        shrunk: 0,
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
        used: FnvHashSet::default(),
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

/// Records the painting candidates of one building's textured walls. Runs
/// after the wall ring is built, so the wall blocks are in place and the air
/// in front of them is known; nothing is hung until `finalize`. Every open
/// outward face of every wall cell is considered, grouped by facing into flat
/// runs, and each run's air cells are reserved so decals and later candidates
/// keep out of them. Returns the number of candidates recorded.
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
    if !s.paintings || !editor.map_decals_enabled() {
        return 0;
    }
    let Some(columns) = s.way_cells.get(&element_id) else {
        return 0;
    };
    let own_wall = |x: i32, z: i32| {
        s.cells
            .get(&(x, z))
            .is_some_and(|c| s.walls[c.wall as usize].way_id == element_id)
    };

    // Open faces by wall and facing. A cell two walls projected onto (the
    // pieces of one wall overlapping) belongs to the last one, the same rule
    // `block_at` follows. A face is open when no wall cell of this building
    // sits in front of it; on a diagonal wall that leaves both faces of every
    // step.
    let mut groups = FaceGroups::new();
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
        let dir = s.wall_dir[cell.wall as usize];
        for (fx, fz) in outward_faces(dir, i32::from(cell.nx), i32::from(cell.nz)) {
            if own_wall(bx + fx, bz + fz) {
                continue;
            }
            let Some(facing) = facing_for_normal(fx, fz) else {
                continue;
            };
            groups
                .entry((cell.wall, facing))
                .or_default()
                .push((bx, bz, cell.col));
        }
    }
    if groups.is_empty() {
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

    for ((wi, facing), cells) in groups {
        let wall = &s.walls[wi as usize];
        let (nx, nz) = normal_for_facing(facing);
        // The textured rows, in blocks, capped by the wall that was built.
        let total_h = ((wall.rows as f64 * scale).round() as i32).min(building_height);
        if total_h <= 0 {
            continue;
        }

        // Rows each column can carry: open air in front, above the terrain and
        // not yet taken by a painting or frame, and a full block (or a pane,
        // made full below) behind. The longest unbroken stretch wins; slopes
        // and passages cut the bottom, eaves the top. Two faces of different
        // cells can open into one air cell at an inner corner; the facing
        // handled first keeps it.
        let columns: Vec<Column> = cells
            .iter()
            .map(|&(bx, bz, col)| {
                let (fx, fz) = (bx + nx, bz + nz);
                let ground = editor.get_absolute_y(fx, 0, fz);
                let usable = |r: i32| {
                    let y = base_y + r;
                    y - ground >= 1
                        && is_clear(editor.get_block_absolute(fx, y, fz))
                        && !editor.cell_has_frame(fx, y, fz)
                        && !registry.used.contains(&(fx, y, fz))
                        && editor
                            .get_block_absolute(bx, y, bz)
                            .is_some_and(|b| is_full_solid(b) || pane_to_full(b).is_some())
                };
                let (lo, hi) = longest_true_range(total_h, usable);
                Column {
                    bx,
                    bz,
                    col,
                    lo,
                    hi,
                }
            })
            .collect();

        // Air cells an earlier pass claimed (another tile building this same
        // wall) are reserved here too, so this tile's decals keep out of them.
        let taken: Vec<(i32, i32, i32)> = cells
            .iter()
            .flat_map(|&(bx, bz, _)| (0..total_h).map(move |r| (bx + nx, base_y + r, bz + nz)))
            .filter(|c| registry.used.contains(c))
            .collect();
        editor.reserve_painting_cells(taken);

        for run in runs(columns, facing) {
            let (lo, hi) = (run[0].lo, run[0].hi);
            let mut air = Vec::with_capacity(run.len() * (hi - lo) as usize);
            let mut regions: Vec<(i32, i32)> = Vec::with_capacity(2);
            for c in &run {
                for y in base_y + lo..base_y + hi {
                    // A pane reads as solid to the game, but only a full block
                    // is a safe backing; the finished-world check wants one.
                    if let Some(full) = editor
                        .get_block_absolute(c.bx, y, c.bz)
                        .and_then(pane_to_full)
                    {
                        editor.set_block_absolute(full, c.bx, y, c.bz, None, Some(&[]));
                    }
                    air.push((c.bx + nx, y, c.bz + nz));
                }
                for key in [(c.bx >> 9, c.bz >> 9), ((c.bx + nx) >> 9, (c.bz + nz) >> 9)] {
                    if !regions.contains(&key) {
                        regions.push(key);
                    }
                }
            }
            registry.used.extend(air.iter().copied());
            editor.reserve_painting_cells(air);
            let index = registry.candidates.len();
            for key in regions {
                registry.by_region.entry(key).or_default().push(index);
            }
            registry.candidates.push(Some(Candidate {
                way_id: element_id,
                wall: wi,
                facing,
                base_y,
                lo,
                hi,
                cells: run.iter().map(|c| (c.bx, c.bz, c.col)).collect(),
            }));
            registry.stats.candidates += 1;
            collected += 1;
        }
    }
    collected
}

/// Variant name of a panel, unique because no two paintings share an air
/// cell. Resource paths take lowercase letters, digits and underscores, so a
/// negative coordinate is spelled with a leading `n`.
fn panel_name(way_id: u64, wall: u32, facing: i8, ax: i32, ay: i32, az: i32) -> String {
    fn coord(v: i32) -> String {
        if v < 0 {
            format!("n{}", v.unsigned_abs())
        } else {
            v.to_string()
        }
    }
    format!(
        "f{way_id}_{wall}_{facing}_{}_{}_{}",
        coord(ax),
        coord(ay),
        coord(az)
    )
}

/// Checks one collected run against the finished world and hangs what still
/// holds: per column the longest stretch of rows with clear air in front and
/// a full solid block behind, grouped into flat rectangles and cut into
/// panels of at most 16 x 16 blocks, each cropped out of the wall texture.
fn place_candidate(editor: &mut WorldEditor, s: &FacadeStore, cand: Candidate, r: &mut Registry) {
    let facing = cand.facing;
    let (nx, nz) = normal_for_facing(facing);
    let wall = &s.walls[cand.wall as usize];
    let n = cand.cells.len() as i32;
    let total = n * (cand.hi - cand.lo);
    let Some(tex) = wall.tex.as_ref().filter(|_| total > 0) else {
        r.stats.dropped += 1;
        return;
    };
    let scale = s.scale;
    let flip = flip_crop(s.wall_dir[cand.wall as usize], facing);
    let fallback = s.building_colour(cand.way_id);
    let (rx, rz) = right_vector(facing);
    let along0 = cand.cells[0].0 * rx + cand.cells[0].1 * rz;
    // The run covers whole texture columns; a diagonal wall packs more
    // metres into fewer blocks, and the crop is squeezed to match.
    let (cmin, cmax) = cand
        .cells
        .iter()
        .fold((u16::MAX, 0u16), |(a, b), c| (a.min(c.2), b.max(c.2)));
    let span_m = f64::from(cmax - cmin + 1);

    let columns: Vec<Column> = cand
        .cells
        .iter()
        .map(|&(bx, bz, col)| {
            let (fx, fz) = (bx + nx, bz + nz);
            let ok = |i: i32| {
                let y = cand.base_y + cand.lo + i;
                is_clear(editor.get_block_absolute(fx, y, fz))
                    && editor
                        .get_block_absolute(bx, y, bz)
                        .is_some_and(is_full_solid)
            };
            let (lo, hi) = longest_true_range(cand.hi - cand.lo, ok);
            Column {
                bx,
                bz,
                col,
                lo: cand.lo + lo,
                hi: cand.lo + hi,
            }
        })
        .collect();
    let valid: i32 = columns.iter().map(|c| c.hi - c.lo).sum();

    let mut hung = 0usize;
    for run in runs(columns, facing) {
        // Position of this rectangle within the collected run, for the crop.
        let k_start = run[0].bx * rx + run[0].bz * rz - along0;
        let (lo, hi) = (run[0].lo, run[0].hi);
        for (p0, p1) in cut(run.len() as i32, MAX_PANEL) {
            let (k0, k1) = (k_start + p0, k_start + p1);
            let w = k1 - k0;
            for (b0, b1) in cut(hi - lo, MAX_PANEL) {
                let (b0, b1) = (lo + b0, lo + b1);
                let h = b1 - b0;
                // Metres along node A to node B at the panel's viewer's-left
                // and viewer's-right edges.
                let (ua, ub) = if flip {
                    (
                        f64::from(cmax) + 1.0 - span_m * f64::from(k1) / f64::from(n),
                        f64::from(cmax) + 1.0 - span_m * f64::from(k0) / f64::from(n),
                    )
                } else {
                    (
                        f64::from(cmin) + span_m * f64::from(k0) / f64::from(n),
                        f64::from(cmin) + span_m * f64::from(k1) / f64::from(n),
                    )
                };
                // Metres down from the top of the texture.
                let va = f64::from(wall.rows) - f64::from(b1) / scale;
                let vb = f64::from(wall.rows) - f64::from(b0) / scale;
                let Some(crop) = crop_texture(tex, ua, ub, va, vb, flip, fallback) else {
                    continue;
                };

                let left = &run[p0 as usize];
                let (lx, lz) = (left.bx + nx, left.bz + nz);
                let (ax, ay, az) = anchor(lx, cand.base_y + b0, lz, facing, w, h);
                let name = panel_name(cand.way_id, cand.wall, facing, ax, ay, az);
                if !editor.add_painting(ax, ay, az, facing, &format!("{NAMESPACE}:{name}")) {
                    continue;
                }
                editor.reserve_painting_cells(covered_cells(ax, ay, az, facing, w, h));
                r.panels.push(Panel {
                    name,
                    w: w as u32,
                    h: h as u32,
                    tex: crop,
                });
                hung += 1;
            }
        }
    }
    r.stats.paintings += hung;
    if hung == 0 {
        r.stats.dropped += 1;
    } else if valid < total {
        r.stats.shrunk += 1;
    } else {
        r.stats.intact += 1;
    }
}

/// Settles the candidates at `indices` that are still pending.
fn place_pending(editor: &mut WorldEditor, indices: impl IntoIterator<Item = usize>) {
    let Some(s) = facades::store() else {
        return;
    };
    let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    if !r.enabled {
        return;
    }
    for i in indices {
        let Some(cand) = r.candidates.get_mut(i).and_then(Option::take) else {
            continue;
        };
        place_candidate(editor, &s, cand, &mut r);
    }
}

/// Settles the pending candidates whose cells touch region `(rx, rz)`. Under
/// stream-to-disk eviction a region's blocks are final when it is flushed,
/// and an entity written after that would be lost, so this runs right before
/// the flush. A candidate reaching into a neighbour is settled at the first
/// of its regions to flush, while every cell it reads is still in memory.
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
/// before it is saved, and reports the outcome to the GUI. `None` when the
/// run collected nothing.
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

/// Summary of what `finalize` hung.
pub struct PlacementReport {
    pub stats: PlacementStats,
}

impl PlacementReport {
    fn summary(&self) -> String {
        let s = self.stats;
        format!(
            "Facade paintings: {} hung from {} candidates ({} as collected, {} shrunk, {} dropped)",
            s.paintings, s.candidates, s.intact, s.shrunk, s.dropped
        )
    }
}

impl fmt::Display for PlacementReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "  {}", self.summary())
    }
}

/// Crops metres [ua, ub) along the wall and [va, vb) down from the top out of
/// the 8 px/m wall texture, mirrors it when asked and fills missing pixels
/// with the wall colour around them (`fill_missing`, with `fallback` as the
/// last resort). None when too little of the crop carries texture.
pub(super) fn crop_texture(
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
            "  Facade paintings: {} paintings at {} px per block in datapacks/{DATAPACK_NAME} and resources.zip",
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

/// Writes the hung panels as a data pack plus resource pack into the world
/// folder and enables the data pack in level.dat. `Ok(None)` when the run
/// hung no paintings.
pub fn write_packs(world_path: &Path) -> Result<Option<PackReport>, String> {
    let (panels, px) = {
        let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled || r.panels.is_empty() {
            return Ok(None);
        }
        (std::mem::take(&mut r.panels), r.px)
    };
    emit_gui_progress_update(98.0, "Writing facade paintings...");
    write_packs_for(world_path, &panels, px).map(Some)
}

/// Total painting texture area at `px` pixels per block.
fn atlas_area(panels: &[Panel], px: u32) -> u64 {
    panels
        .iter()
        .map(|p| u64::from(p.w * px) * u64::from(p.h * px))
        .sum()
}

/// Halves the resolution until the atlas budget holds, down to 4 px per block.
fn fit_px(panels: &[Panel], requested_px: u32) -> u32 {
    let mut px = requested_px.max(MIN_PX_PER_BLOCK);
    // One step at a time, not `px /= 2`: halving is a four times jump in
    // area, and on a real box it lands at 8 where 11 would have fitted. The
    // atlas only cares that a sprite side is a multiple of 16, which
    // `tex_side` gives for any integer px, so nothing here wants a power of
    // two.
    while px > MIN_PX_PER_BLOCK && atlas_area(panels, px) > atlas_budget() {
        px -= 1;
    }
    px
}

/// pack.mcmeta accepted by every 1.21.x and later: the old keys for 1.21 to
/// 1.21.8, `min_format`/`max_format` for 1.21.9+, each open-ended upwards.
pub fn pack_mcmeta(pack_format: u32) -> String {
    pack_mcmeta_described(pack_format, "Arnis facade paintings")
}

/// `pack_mcmeta` with the pack's own description, so the display panels do not
/// have to call themselves paintings in the pack list.
pub(super) fn pack_mcmeta_described(pack_format: u32, description: &str) -> String {
    serde_json::json!({
        "pack": {
            "pack_format": pack_format,
            "supported_formats": [pack_format, 999],
            "min_format": pack_format,
            "max_format": [999, 0],
            "description": description
        }
    })
    .to_string()
}

/// A painting variant definition. No title or author: they are 1.21.2+ only
/// and would show in the tooltip.
pub fn variant_json(name: &str, w: u32, h: u32) -> String {
    serde_json::json!({
        "asset_id": format!("{NAMESPACE}:{name}"),
        "width": w,
        "height": h
    })
    .to_string()
}

/// The resource pack as zip bytes: pack.mcmeta and one PNG per variant.
fn resource_zip(panels: &[&Panel], px: u32) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    // A PNG carries deflated pixels already; deflating it again is the most
    // expensive thing this writer does and shrinks the pack by a few per cent.
    let png_options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("pack.mcmeta", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(pack_mcmeta(RESOURCEPACK_FORMAT).as_bytes())
        .map_err(|e| e.to_string())?;
    // Marks the pack as ours, so installing over it never has to guess.
    zip.start_file(super::displays::PACK_MARKER, options)
        .map_err(|e| e.to_string())?;
    zip.write_all(
        b"arnis facade paintings
",
    )
    .map_err(|e| e.to_string())?;
    for p in panels {
        let img = image::imageops::resize(
            &p.tex,
            p.w * px,
            p.h * px,
            image::imageops::FilterType::Triangle,
        );
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("encode {}: {e}", p.name))?;
        zip.start_file(
            format!("assets/{NAMESPACE}/textures/painting/{}.png", p.name),
            png_options,
        )
        .map_err(|e| e.to_string())?;
        zip.write_all(&png).map_err(|e| e.to_string())?;
    }
    let cursor = zip.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

/// Writes the packs for `panels` into `world_path`, at `requested_px` pixels
/// per block or the highest resolution below it that fits the atlas.
fn write_packs_for(
    world_path: &Path,
    panels: &[Panel],
    requested_px: u32,
) -> Result<PackReport, String> {
    let px = fit_px(panels, requested_px);
    if px < requested_px {
        warn(&format!(
            "Facade paintings: {} panels would not fit the game's painting atlas at {requested_px} px per block; using {px} px.",
            panels.len()
        ));
    }
    if atlas_area(panels, px) > atlas_budget() {
        warn(&format!(
            "Facade paintings: {} panels exceed the painting atlas even at {px} px per block; the game may fail to stitch them.",
            panels.len()
        ));
    }
    let mut panels: Vec<&Panel> = panels.iter().collect();
    panels.sort_by(|a, b| a.name.cmp(&b.name));

    // Data pack: one JSON per variant, replacing whatever an earlier run left.
    let dp_root = world_path.join("datapacks").join(DATAPACK_NAME);
    let variants = dp_root
        .join("data")
        .join(NAMESPACE)
        .join("painting_variant");
    if variants.exists() {
        std::fs::remove_dir_all(&variants)
            .map_err(|e| format!("clear {}: {e}", variants.display()))?;
    }
    std::fs::create_dir_all(&variants)
        .map_err(|e| format!("create {}: {e}", variants.display()))?;
    std::fs::write(dp_root.join("pack.mcmeta"), pack_mcmeta(DATAPACK_FORMAT))
        .map_err(|e| format!("write pack.mcmeta: {e}"))?;
    for p in &panels {
        let path = variants.join(format!("{}.json", p.name));
        std::fs::write(&path, variant_json(&p.name, p.w, p.h))
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }

    // Resource pack: the same zip at both places the game has looked for it.
    let bytes = resource_zip(&panels, px)?;
    let rp_dir = world_path.join("resourcepacks");
    std::fs::create_dir_all(&rp_dir).map_err(|e| format!("create {}: {e}", rp_dir.display()))?;
    for path in [
        world_path.join("resources.zip"),
        rp_dir.join("resources.zip"),
    ] {
        super::displays::write_world_pack(&path, &bytes)?;
    }

    crate::world_utils::enable_datapack_in_level_dat(world_path, DATAPACK_NAME)?;

    Ok(PackReport {
        panels: panels.len(),
        px,
        requested_px,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{
        COBBLESTONE_WALL, OAK_LEAVES, OAK_LOG, SMOOTH_STONE, SMOOTH_STONE_SLAB, STONE, WHITE_CARPET,
    };
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::buildings::relation_ring_id;
    use crate::mapillary::facades::{CellRef, FacadeWall};
    use crate::osm_parser::ProcessedElement;
    use fnv::FnvHashMap;
    use image::Rgb;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// The placement tests share the process-wide facade store and registry
    /// with the `displays.rs` ones, so they all run one at a time.
    use crate::mapillary::facades::TEST_GLOBALS as GLOBALS;

    #[test]
    fn facing_bytes_match_the_game() {
        assert_eq!(facing_for_normal(0, 1), Some(0), "south");
        assert_eq!(facing_for_normal(-1, 0), Some(1), "west");
        assert_eq!(facing_for_normal(0, -1), Some(2), "north");
        assert_eq!(facing_for_normal(1, 0), Some(3), "east");
        assert_eq!(facing_for_normal(0, 0), None);
        for facing in 0..4 {
            let (nx, nz) = normal_for_facing(facing);
            assert_eq!(facing_for_normal(nx, nz), Some(facing));
        }
    }

    #[test]
    fn outward_faces_follow_the_true_normal() {
        // A wall along x has the one face its snapped normal names.
        assert_eq!(outward_faces((19, 0), 0, 1), vec![(0, 1)]);
        assert_eq!(outward_faces((19, 0), 0, -1), vec![(0, -1)]);
        assert_eq!(outward_faces((0, 12), 1, 0), vec![(1, 0)]);
        assert_eq!(outward_faces((0, -12), -1, 0), vec![(-1, 0)]);
        // At 45 degrees both components count, whichever axis the snap chose.
        assert_eq!(outward_faces((7, 7), 1, 0), vec![(1, 0), (0, -1)]);
        assert_eq!(outward_faces((7, 7), 0, -1), vec![(1, 0), (0, -1)]);
        assert_eq!(outward_faces((7, 7), -1, 0), vec![(-1, 0), (0, 1)]);
        assert_eq!(outward_faces((-7, 7), 1, 0), vec![(1, 0), (0, 1)]);
        // A wall a few degrees off the axis still has a second face, for its
        // rare steps.
        assert_eq!(outward_faces((20, 3), 0, -1), vec![(1, 0), (0, -1)]);
        assert_eq!(outward_faces((20, -3), 0, 1), vec![(1, 0), (0, 1)]);
        // Without a direction the snapped normal is all there is.
        assert_eq!(outward_faces((0, 0), 0, 1), vec![(0, 1)]);
    }

    #[test]
    fn anchor_covers_exactly_the_panel_for_every_facing_and_parity() {
        let (lx, by, lz) = (100, 64, -40);
        for facing in 0..4i8 {
            let (rx, rz) = right_vector(facing);
            for (w, h) in [
                (1, 1),
                (2, 1),
                (1, 2),
                (2, 2),
                (3, 3),
                (4, 3),
                (3, 4),
                (15, 16),
                (16, 15),
                (16, 16),
            ] {
                let (ax, ay, az) = anchor(lx, by, lz, facing, w, h);
                let expected: BTreeSet<(i32, i32, i32)> = (0..w)
                    .flat_map(|k| (0..h).map(move |r| (lx + rx * k, by + r, lz + rz * k)))
                    .collect();
                let got: BTreeSet<(i32, i32, i32)> = covered_cells(ax, ay, az, facing, w, h)
                    .into_iter()
                    .collect();
                assert_eq!(got, expected, "facing {facing} w {w} h {h}");
                assert!(got.contains(&(ax, ay, az)), "anchor lies inside the panel");
            }
        }
        // Spot check against the game's rule: a 2 x 2 facing south leans east and up.
        assert_eq!(anchor(10, 5, 20, 0, 2, 2), (10, 5, 20));
        assert_eq!(
            covered_cells(10, 5, 20, 0, 2, 2),
            vec![(10, 5, 20), (10, 6, 20), (11, 5, 20), (11, 6, 20)]
        );
        // A 3 x 3 sits centred on its anchor.
        assert_eq!(anchor(10, 5, 20, 0, 3, 3), (11, 6, 20));
    }

    #[test]
    fn runs_break_at_every_step_of_a_staircase() {
        // A wall climbing one block in z every few blocks in x, facing south,
        // with a gap at x = 6.
        let col = |bx, bz| Column {
            bx,
            bz,
            col: bx as u16,
            lo: 0,
            hi: 5,
        };
        let cells = vec![
            col(3, 1),
            col(0, 0),
            col(7, 2),
            col(1, 0),
            col(5, 2),
            col(2, 0),
            col(4, 1),
        ];
        let shape: Vec<Vec<(i32, i32)>> = runs(cells, 0)
            .iter()
            .map(|r| r.iter().map(|c| (c.bx, c.bz)).collect())
            .collect();
        assert_eq!(
            shape,
            vec![
                vec![(0, 0), (1, 0), (2, 0)],
                vec![(3, 1), (4, 1)],
                vec![(5, 2)],
                vec![(7, 2)],
            ]
        );
    }

    #[test]
    fn runs_follow_the_viewers_right_and_split_on_usable_rows() {
        // Facing north the viewer's right is west, so runs go down in x.
        let cells = vec![
            Column {
                bx: 0,
                bz: 0,
                col: 0,
                lo: 0,
                hi: 4,
            },
            Column {
                bx: 1,
                bz: 0,
                col: 1,
                lo: 0,
                hi: 4,
            },
            Column {
                bx: 2,
                bz: 0,
                col: 2,
                lo: 1,
                hi: 4,
            },
        ];
        let r = runs(cells, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].iter().map(|c| c.bx).collect::<Vec<_>>(), vec![2]);
        assert_eq!(r[1].iter().map(|c| c.bx).collect::<Vec<_>>(), vec![1, 0]);
        // Columns without usable rows vanish.
        assert!(runs(
            vec![Column {
                bx: 0,
                bz: 0,
                col: 0,
                lo: 3,
                hi: 3
            }],
            0
        )
        .is_empty());
    }

    #[test]
    fn crop_is_mirrored_when_node_a_is_on_the_viewers_right() {
        // Node A to node B runs east.
        assert!(!flip_crop((10, 0), 0), "facing south, right is east");
        assert!(flip_crop((10, 0), 2), "facing north, right is west");
        // Node A to node B runs south.
        assert!(flip_crop((0, 10), 3), "facing east, right is north");
        assert!(!flip_crop((0, 10), 1), "facing west, right is south");
        assert!(!flip_crop((0, 0), 0));
    }

    #[test]
    fn panels_cut_at_sixteen() {
        assert_eq!(cut(17, MAX_PANEL), vec![(0, 16), (16, 17)]);
        assert_eq!(cut(16, MAX_PANEL), vec![(0, 16)]);
        assert_eq!(cut(33, MAX_PANEL), vec![(0, 16), (16, 32), (32, 33)]);
        assert!(cut(0, MAX_PANEL).is_empty());
    }

    #[test]
    fn longest_usable_stretch_prefers_the_lowest_on_ties() {
        assert_eq!(longest_true_range(6, |r| r != 3), (0, 3));
        assert_eq!(longest_true_range(5, |r| r >= 2), (2, 5));
        assert_eq!(longest_true_range(5, |_| false), (0, 0));
        assert_eq!(longest_true_range(4, |r| r != 1 && r != 2), (0, 1));
        assert_eq!(longest_true_range(7, |r| r > 1), (2, 7));
    }

    #[test]
    fn backing_wants_a_full_block_and_air_cells_want_nothing_that_collides() {
        for full in [
            STONE,
            SMOOTH_STONE,
            GLASS,
            GRAY_STAINED_GLASS,
            OAK_LOG,
            OAK_LEAVES,
        ] {
            assert!(is_full_solid(full), "{}", full.name());
        }
        for partial in [
            AIR,
            GLASS_PANE,
            GRAY_STAINED_GLASS_PANE,
            SMOOTH_STONE_SLAB,
            COBBLESTONE_WALL,
            WHITE_CARPET,
        ] {
            assert!(!is_full_solid(partial), "{}", partial.name());
        }
        assert!(is_clear(None));
        assert!(is_clear(Some(AIR)));
        assert!(!is_clear(Some(WHITE_CARPET)), "a carpet collides");
        assert!(!is_clear(Some(OAK_LEAVES)));
        assert!(!is_clear(Some(GLASS_PANE)));
    }

    #[test]
    fn panel_names_are_valid_resource_paths() {
        assert_eq!(panel_name(4242, 1, 2, 102, 3, 39), "f4242_1_2_102_3_39");
        assert_eq!(panel_name(7, 0, 3, -5, 64, -120), "f7_0_3_n5_64_n120");
        assert!(panel_name(1, 2, 3, i32::MIN, 0, 0)
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
    }

    #[test]
    fn pack_mcmeta_has_the_shape_every_1_21_accepts() {
        let v: serde_json::Value = serde_json::from_str(&pack_mcmeta(48)).unwrap();
        assert_eq!(v["pack"]["pack_format"], 48);
        assert_eq!(v["pack"]["supported_formats"], serde_json::json!([48, 999]));
        assert_eq!(v["pack"]["min_format"], 48);
        assert_eq!(v["pack"]["max_format"], serde_json::json!([999, 0]));
        assert!(v["pack"]["description"].is_string());
        assert!(v.get("overlays").is_none());

        let v: serde_json::Value = serde_json::from_str(&variant_json("f1_2_3", 16, 9)).unwrap();
        assert_eq!(v["asset_id"], "arnis:f1_2_3");
        assert_eq!(v["width"], 16);
        assert_eq!(v["height"], 9);
        assert!(v.get("title").is_none());
        assert!(v.get("author").is_none());
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

    #[test]
    fn fit_px_steps_down_to_the_largest_size_the_atlas_budget_holds() {
        let panel = |i: usize| Panel {
            name: format!("p{i}"),
            w: 16,
            h: 16,
            tex: RgbImage::new(1, 1),
        };
        // 1000 full panels: 65.5 M px at 16, and the budget holds 12.
        let many: Vec<Panel> = (0..1000).map(panel).collect();
        assert_eq!(fit_px(&many, 16), 12);
        assert_eq!(fit_px(&many, 32), 12);
        assert!(atlas_area(&many, 12) <= atlas_budget());
        assert!(
            atlas_area(&many, 13) > atlas_budget(),
            "12 must be the largest that fits"
        );
        assert_eq!(fit_px(&many[..10], 16), 16);
        assert_eq!(fit_px(&many[..10], 32), 32);
        // Never below 4, even when that still does not fit.
        let huge: Vec<Panel> = (0..20_000).map(panel).collect();
        assert_eq!(fit_px(&huge, 16), 4);
    }

    #[test]
    fn packs_land_in_the_world_folder_and_level_dat() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        let panels = vec![Panel {
            name: "f1_0_0".to_string(),
            w: 3,
            h: 2,
            tex: RgbImage::from_pixel(24, 16, Rgb([10, 20, 30])),
        }];
        let report = write_packs_for(&world, &panels, 16).unwrap();
        assert_eq!((report.panels, report.px), (1, 16));

        let dp = world.join("datapacks").join(DATAPACK_NAME);
        let mcmeta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dp.join("pack.mcmeta")).unwrap())
                .unwrap();
        assert_eq!(mcmeta["pack"]["pack_format"], 48);
        let variant: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dp.join("data/arnis/painting_variant/f1_0_0.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(variant["asset_id"], "arnis:f1_0_0");
        assert_eq!(variant["width"], 3);

        for rel in ["resources.zip", "resourcepacks/resources.zip"] {
            let file = std::fs::File::open(world.join(rel)).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let mut text = String::new();
            std::io::Read::read_to_string(&mut archive.by_name("pack.mcmeta").unwrap(), &mut text)
                .unwrap();
            let mcmeta: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(mcmeta["pack"]["pack_format"], 34, "{rel}");
            let mut png = Vec::new();
            std::io::Read::read_to_end(
                &mut archive
                    .by_name("assets/arnis/textures/painting/f1_0_0.png")
                    .unwrap(),
                &mut png,
            )
            .unwrap();
            let img = image::load_from_memory(&png).unwrap();
            assert_eq!((img.width(), img.height()), (48, 32), "{rel}");
        }

        let raw = std::fs::read(world.join("level.dat")).unwrap();
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(
            &mut flate2::read::GzDecoder::new(raw.as_slice()),
            &mut decompressed,
        )
        .unwrap();
        let root: fastnbt::Value = fastnbt::from_bytes(&decompressed).unwrap();
        let fastnbt::Value::Compound(root) = root else {
            panic!("root not a compound");
        };
        let Some(fastnbt::Value::Compound(data)) = root.get("Data") else {
            panic!("missing Data");
        };
        let Some(fastnbt::Value::Compound(packs)) = data.get("DataPacks") else {
            panic!("missing DataPacks");
        };
        let Some(fastnbt::Value::List(enabled)) = packs.get("Enabled") else {
            panic!("missing Enabled");
        };
        assert!(enabled
            .iter()
            .any(|v| matches!(v, fastnbt::Value::String(s) if s == "file/arnis_facades")));
    }

    /// A `cols` m wide, 6 m high wall texture: red on the node A half, blue
    /// on the node B half.
    fn two_tone_tex(cols: u32) -> RgbaImage {
        let width = cols * TEX_PX_PER_M as u32;
        RgbaImage::from_fn(width, 48, |x, _| {
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

    /// Panel sizes in the registry, by name.
    fn panel_sizes() -> Vec<(String, u32, u32)> {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<_> = r
            .panels
            .iter()
            .map(|p| (p.name.clone(), p.w, p.h))
            .collect();
        v.sort();
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
    fn straight_walls_hang_one_painting_per_panel_with_the_crop_facing_the_viewer() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 4242;
        // Two 20-block walls of one building: one at z = 50 facing south, one
        // at z = 40 facing north, both textured from node A (west) to B (east).
        let walls = vec![
            FacadeWall::for_test(WAY, 1, 2, 20, 6, two_tone_tex(20)),
            FacadeWall::for_test(WAY, 3, 4, 20, 6, two_tone_tex(20)),
        ];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 100, 50, 20, 1);
        wall_along_x(&mut cells, 1, 100, 40, 20, -1);
        facades::install_for_test(walls, cells, vec![(19, 0), (19, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        for i in 0..20 {
            for y in 1..=6 {
                editor.set_block_absolute(SMOOTH_STONE, 100 + i, y, 50, None, None);
                editor.set_block_absolute(SMOOTH_STONE, 100 + i, y, 40, None, None);
            }
        }

        reset(true, 16);
        // One run per wall; a straight wall has a single face.
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 2);
        // Nothing hangs yet, but the air cells are taken: a frame can no
        // longer go where a painting will, and a second pass finds no room.
        assert!(editor.paintings().is_empty());
        assert!(editor.cell_has_frame(100, 1, 51));
        assert!(editor.cell_has_frame(119, 6, 51));
        assert!(editor.cell_has_frame(104, 6, 39));
        assert!(!editor.cell_has_frame(100, 7, 51));
        assert!(!editor.place_map_decal_ex(110, 3, 50, 3, 1, 0, false, true));
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 0);

        let report = finalize(&mut editor).unwrap();
        assert_eq!(
            report.stats,
            PlacementStats {
                candidates: 2,
                paintings: 4,
                intact: 2,
                shrunk: 0,
                dropped: 0,
            }
        );
        let mut paintings = editor.paintings();
        paintings.sort();
        assert_eq!(
            paintings,
            vec![
                // North wall (z = 39), viewer's right is west: the 16-wide
                // panel takes x 119 down to 104, the 4-wide one 103 to 100,
                // each anchored floor((w - 1) / 2) to the viewer's right.
                (102, 3, 39, 2, "arnis:f4242_1_2_102_3_39".to_string()),
                // South wall (z = 51), viewer's right is east: x 100 to 115
                // anchored at 107, and 116 to 119 anchored at 117.
                (107, 3, 51, 0, "arnis:f4242_0_0_107_3_51".to_string()),
                (112, 3, 39, 2, "arnis:f4242_1_2_112_3_39".to_string()),
                (117, 3, 51, 0, "arnis:f4242_0_0_117_3_51".to_string()),
            ]
        );
        assert_eq!(
            panel_sizes(),
            vec![
                ("f4242_0_0_107_3_51".to_string(), 16, 6),
                ("f4242_0_0_117_3_51".to_string(), 4, 6),
                ("f4242_1_2_102_3_39".to_string(), 4, 6),
                ("f4242_1_2_112_3_39".to_string(), 16, 6),
            ]
        );
        // Seen from the south, node A (red) is on the viewer's left.
        assert_eq!(
            panel_corners("f4242_0_0_107_3_51"),
            ((128, 48), [255, 0, 0], [0, 0, 255])
        );
        // Seen from the north the wall is mirrored: node B (blue) is left.
        assert_eq!(
            panel_corners("f4242_1_2_112_3_39"),
            ((128, 48), [0, 0, 255], [255, 0, 0])
        );
        // The short south panel is the last 4 m: all blue.
        assert_eq!(
            panel_corners("f4242_0_0_117_3_51"),
            ((32, 48), [0, 0, 255], [0, 0, 255])
        );

        // Settling again finds nothing pending.
        assert!(finalize(&mut editor).is_some());
        assert_eq!(editor.paintings().len(), 4);
        assert_eq!(stats().paintings, 4);
    }

    #[test]
    fn a_diagonal_wall_gets_panels_on_both_faces_of_its_steps() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 4343;
        // A 45 degree wall from node A at (100, 50) to node B at (107, 57):
        // eight cells, one per Bresenham step, one texture column each. The
        // building lies to the south-west, so the true normal points
        // north-east; the snap picked east.
        let walls = vec![FacadeWall::for_test(WAY, 11, 12, 8, 6, two_tone_tex(8))];
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
        facades::install_for_test(walls, cells, vec![(7, 7)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        for i in 0..8 {
            for y in 1..=6 {
                editor.set_block_absolute(SMOOTH_STONE, 100 + i, y, 50 + i, None, None);
            }
        }

        reset(true, 16);
        // Every step's north face, plus the east face of the last cell. The
        // other east faces open into the air cell that the next step's north
        // face already took: two paintings never share a cell.
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 9);
        let report = finalize(&mut editor).unwrap();
        assert_eq!(
            report.stats,
            PlacementStats {
                candidates: 9,
                paintings: 9,
                intact: 9,
                shrunk: 0,
                dropped: 0,
            }
        );
        let mut paintings = editor.paintings();
        paintings.sort();
        let mut expected: Vec<(i32, i32, i32, i8, String)> = (0..8)
            .map(|i| {
                let (x, z) = (100 + i, 49 + i);
                (x, 3, z, 2, format!("arnis:f4343_0_2_{x}_3_{z}"))
            })
            .collect();
        expected.push((108, 3, 57, 3, "arnis:f4343_0_3_108_3_57".to_string()));
        expected.sort();
        assert_eq!(paintings, expected);

        // Each 1-wide panel shows its own cell's texture column, mirrored
        // because node A is on the viewer's right from the north and from the
        // east: cell 0 is on the red half, cell 7 on the blue one.
        let sizes = panel_sizes();
        assert_eq!(sizes.len(), 9);
        assert!(sizes.iter().all(|(_, w, h)| (*w, *h) == (1, 6)));
        assert_eq!(
            panel_corners("f4343_0_2_100_3_49"),
            ((8, 48), [255, 0, 0], [255, 0, 0])
        );
        assert_eq!(
            panel_corners("f4343_0_2_107_3_56"),
            ((8, 48), [0, 0, 255], [0, 0, 255])
        );
        assert_eq!(
            panel_corners("f4343_0_3_108_3_57"),
            ((8, 48), [0, 0, 255], [0, 0, 255])
        );
    }

    #[test]
    fn finalize_drops_a_blocked_panel_and_shrinks_one_that_lost_its_backing() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 4444;
        // Two south-facing walls of one building: three cells at z = 50 and
        // twenty at z = 40.
        let walls = vec![
            FacadeWall::for_test(WAY, 21, 22, 3, 6, two_tone_tex(3)),
            FacadeWall::for_test(WAY, 23, 24, 20, 6, two_tone_tex(20)),
        ];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 100, 50, 3, 1);
        wall_along_x(&mut cells, 1, 100, 40, 20, 1);
        facades::install_for_test(walls, cells, vec![(2, 0), (19, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        for y in 1..=6 {
            for i in 0..3 {
                editor.set_block_absolute(SMOOTH_STONE, 100 + i, y, 50, None, None);
            }
            for i in 0..20 {
                editor.set_block_absolute(SMOOTH_STONE, 100 + i, y, 40, None, None);
            }
        }

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 2);

        // Later processing: a tree's crown fills the air in front of the short
        // wall, and a roof pass swaps the long wall's top row for slabs and
        // panes. Glass in the row below is still a full block.
        for i in 0..3 {
            for y in 1..=6 {
                editor.set_block_absolute(OAK_LEAVES, 100 + i, y, 51, None, Some(&[]));
            }
        }
        for i in 0..20 {
            let top = if i < 10 {
                SMOOTH_STONE_SLAB
            } else {
                GLASS_PANE
            };
            editor.set_block_absolute(top, 100 + i, 6, 40, None, Some(&[]));
            editor.set_block_absolute(GLASS, 100 + i, 5, 40, None, Some(&[]));
        }

        let report = finalize(&mut editor).unwrap();
        assert_eq!(
            report.stats,
            PlacementStats {
                candidates: 2,
                paintings: 2,
                intact: 0,
                shrunk: 1,
                dropped: 1,
            }
        );
        let mut paintings = editor.paintings();
        paintings.sort();
        // Five rows survive on the long wall: the anchor is floor(4 / 2) = 2
        // rows above the first wall block.
        assert_eq!(
            paintings,
            vec![
                (107, 3, 41, 0, "arnis:f4444_1_0_107_3_41".to_string()),
                (117, 3, 41, 0, "arnis:f4444_1_0_117_3_41".to_string()),
            ]
        );
        assert_eq!(
            panel_sizes(),
            vec![
                ("f4444_1_0_107_3_41".to_string(), 16, 5),
                ("f4444_1_0_117_3_41".to_string(), 4, 5),
            ]
        );
        // The crop lost the top metre as well.
        assert_eq!(
            panel_corners("f4444_1_0_107_3_41"),
            ((128, 40), [255, 0, 0], [0, 0, 255])
        );
    }

    #[test]
    fn flush_region_settles_only_the_candidates_touching_that_region() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        const WAY: u64 = 4545;
        // One south-facing wall straddling the region border at x = 512, and
        // one well inside region (0, 0).
        let walls = vec![
            FacadeWall::for_test(WAY, 31, 32, 10, 6, two_tone_tex(10)),
            FacadeWall::for_test(WAY, 33, 34, 4, 6, two_tone_tex(4)),
        ];
        let mut cells: FnvHashMap<(i32, i32), CellRef> = FnvHashMap::default();
        wall_along_x(&mut cells, 0, 507, 50, 10, 1);
        wall_along_x(&mut cells, 1, 100, 50, 4, 1);
        facades::install_for_test(walls, cells, vec![(9, 0), (3, 0)], 1.0);

        let xzbbox = XZBBox::rect_from_xz_lengths(600.0, 200.0).unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        for y in 1..=6 {
            for i in 0..10 {
                editor.set_block_absolute(SMOOTH_STONE, 507 + i, y, 50, None, None);
            }
            for i in 0..4 {
                editor.set_block_absolute(SMOOTH_STONE, 100 + i, y, 50, None, None);
            }
        }

        reset(true, 16);
        assert_eq!(collect(&mut editor, WAY, 0, 0, 6), 2);
        // Region (1, 0) holds the east end of the long wall and nothing else.
        flush_region(&mut editor, 1, 0);
        assert_eq!(editor.paintings().len(), 1);
        assert_eq!(stats().paintings, 1);
        // Flushing it again, or a region with no candidates, changes nothing.
        flush_region(&mut editor, 1, 0);
        flush_region(&mut editor, 5, 5);
        assert_eq!(editor.paintings().len(), 1);
        // The rest waits for the end.
        let report = finalize(&mut editor).unwrap();
        assert_eq!(report.stats.paintings, 2);
        assert_eq!(report.stats.intact, 2);
        assert_eq!(editor.paintings().len(), 2);
    }

    /// The crop of a registered panel.
    fn panel_texture(name: &str) -> RgbImage {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        r.panels
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .tex
            .clone()
    }

    #[test]
    fn a_wall_merged_over_five_ring_edges_hangs_its_columns_in_ring_order() {
        let _guard = GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        // r147094's south wall as one exported wall of 65 columns over its
        // five OSM edges (see `facades::test_fixtures`), textured with one
        // colour per metre column so a crop tells which columns it holds.
        let relation = facades::test_fixtures::r147094_relation();
        let ring_id = relation_ring_id(147094, 0);
        let column_colour = |m: u32| {
            let m = m as u8;
            [3 * m, 255 - 3 * m, 100]
        };
        let px_per_m = TEX_PX_PER_M as u32;
        let tex = RgbaImage::from_fn(65 * px_per_m, 48, |x, _| {
            let [r, g, b] = column_colour(x / px_per_m);
            image::Rgba([r, g, b, 255])
        });
        // The edges' metres along the wall, each a whole number of blocks on
        // the fixture's outline.
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
        facades::install_elements_for_test(walls, &elements, &xzbbox, 1.0);

        let mut editor = crate::element_processing::building_test_support::test_editor(&xzbbox);
        editor.set_map_decals(true);
        for x in 20..=85 {
            for y in 1..=6 {
                editor.set_block_absolute(SMOOTH_STONE, x, y, 60, None, None);
            }
        }

        reset(true, 16);
        // The 65 textured blocks face south as one flat run; the block at the
        // far corner (x = 85) is past the last column and is the next wall's.
        assert_eq!(collect(&mut editor, ring_id, 0, 0, 6), 1);
        let report = finalize(&mut editor).unwrap();
        assert_eq!(
            report.stats,
            PlacementStats {
                candidates: 1,
                paintings: 5,
                intact: 1,
                shrunk: 0,
                dropped: 0,
            }
        );
        // Four 16-wide panels and one single column, each anchored
        // floor((w - 1) / 2) to the viewer's right (east) of its left edge.
        let name = |ax: i32| format!("f{ring_id}_0_0_{ax}_3_61");
        let mut paintings = editor.paintings();
        paintings.sort();
        assert_eq!(
            paintings,
            vec![
                (27, 3, 61, 0, format!("arnis:{}", name(27))),
                (43, 3, 61, 0, format!("arnis:{}", name(43))),
                (59, 3, 61, 0, format!("arnis:{}", name(59))),
                (75, 3, 61, 0, format!("arnis:{}", name(75))),
                (84, 3, 61, 0, format!("arnis:{}", name(84))),
            ]
        );
        // Each panel holds exactly the metre columns of its blocks, left to
        // right along the ring from node 21486944: no column twice, none out
        // of order, and not mirrored, since node A is on the viewer's left
        // seen from the south.
        for (ax, first_col, w) in [
            (27i32, 0u32, 16u32),
            (43, 16, 16),
            (59, 32, 16),
            (75, 48, 16),
            (84, 64, 1),
        ] {
            let crop = panel_texture(&name(ax));
            assert_eq!((crop.width(), crop.height()), (w * px_per_m, 48));
            for k in 0..w {
                assert_eq!(
                    crop.get_pixel(k * px_per_m + px_per_m / 2, 24).0,
                    column_colour(first_col + k),
                    "panel at x {ax}, block {k}"
                );
            }
        }
    }
}
