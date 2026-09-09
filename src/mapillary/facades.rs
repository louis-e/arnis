//! Facade textures from an orthofacade export, applied to building walls.
//!
//! The export (`tools/facade_lab/export_arnis.py`) holds one JSON per building
//! and, per wall, a PNG at 1 px = 1 m whose RGB is the facade colour and whose
//! alpha is the class (wall / window / door / unknown), plus an 8 px/m texture.
//! Walls are identified by OSM node ids, so the mapping onto the world does not
//! depend on how the lab or Arnis happened to merge or split edges. A way
//! building is keyed by its way id. A relation building is matched through
//! the outer rings the generator assembles from its member ways, and keyed by
//! the synthetic way id `generate_building_from_relation` builds each ring
//! under, because that is the id the wall builder asks with.
//!
//! Two ways to apply a wall:
//! * blocks: every wall cell takes the palette block nearest its colour, window
//!   cells take the style's window block, door cells a plank. This is what the
//!   generator consults from `apply_block_variety`.
//! * photos: blocks as above, plus the 8 px/m texture hung as item display
//!   entities on the wall's true line, one flat quad per wall face whatever
//!   angle the wall runs at (`displays.rs`). Java 1.21.4+ only.
//!
//! Below two blocks per metre a lab cell is at most one block and the lab's
//! window placement does not beat the procedural windows, so only the colours
//! are taken: the building keeps its normal style, its wall block is the lab's
//! building colour and every plain wall block takes the colour of the lab's
//! floor band at its height (`band_block_at`). The full grid, windows and
//! doors included, applies from two blocks per metre up (`block_at`). The
//! photo panels hang either way.
//!
//! Everything is loaded in a pre-pass into a process-wide table, the same way
//! the sampled colours are, so tile threads only read.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::{Arc, RwLock};

use fnv::{FnvHashMap, FnvHashSet};
use image::RgbaImage;
use serde_json::Value;

use crate::args::Args;
use crate::block_definitions::{Block, DARK_OAK_PLANKS};
use crate::block_palette::facade_block_for_color;
use crate::colors::RGBTuple;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::buildings::relation_outer_rings;
use crate::osm_parser::{ProcessedElement, ProcessedNode};

use super::credits::{self, ImageCredit};

/// Class of one facade cell, decoded from the PNG alpha channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Wall,
    Window,
    Door,
    Unknown,
    NoData,
}

impl Class {
    fn from_alpha(a: u8) -> Self {
        match a {
            255 => Class::Wall,
            192 => Class::Window,
            128 => Class::Door,
            64 => Class::Unknown,
            _ => Class::NoData,
        }
    }
}

/// One original OSM edge inside a (possibly merged) lab wall. Columns run
/// from `node_a` towards `node_b`. `span_m` is where the edge lies along the
/// wall, in metres from the start of this piece, the way the export measures
/// it; `col0..col1` is the block-column interval the lab derived from that,
/// used only for exports without the metre span.
#[derive(Clone, Debug)]
struct FacadeEdge {
    node_a: u64,
    node_b: u64,
    col0: i32,
    col1: i32,
    span_m: Option<(f64, f64)>,
}

/// The OSM object a building was exported for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Way(u64),
    Relation(u64),
}

/// `way_id` of a relation wall that lies on none of the rings the generator
/// builds; no cell refers to such a wall, so it is never consulted. Real way
/// ids start at 1 and the synthetic ring ids have bit 63 set.
const UNPLACED: u64 = 0;

/// One exported wall.
pub struct FacadeWall {
    /// The id the generator builds this wall under: the way id, or for a
    /// relation wall the synthetic id of the outer ring it lies on, filled in
    /// when the relation is projected (`UNPLACED` until then).
    pub way_id: u64,
    owner: Owner,
    pub cols: u32,
    pub rows: u32,
    edges: Vec<FacadeEdge>,
    /// Metres from the start of this piece to the left edge of column 0 (the
    /// export's `extent.s_l`). A long wall is exported in pieces of about
    /// 20 m, each with its own texture, and a piece's texture may begin a
    /// little before or after its nominal start.
    col0_m: f64,
    /// Row-major, row 0 at the top of the wall.
    cells: Vec<(RGBTuple, Class)>,
    /// Per row, the colour of the lab's floor band there: the median of the
    /// row's wall cells, or the nearest such row's when the row has none.
    /// Empty when no row has a wall cell.
    bands: Vec<RGBTuple>,
    /// 8 px/m texture for the photo panels, alpha = valid.
    pub(super) tex: Option<RgbaImage>,
}

impl FacadeWall {
    fn cell(&self, col: u32, row: u32) -> Option<(RGBTuple, Class)> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        self.cells.get((row * self.cols + col) as usize).copied()
    }

    /// The colour of the lab's floor band at `row`, see `bands`.
    fn band(&self, row: u32) -> Option<RGBTuple> {
        self.bands.get(row as usize).copied()
    }

    /// A wall of plain grey wall cells with a texture, spanning one edge from
    /// node `node_a` to node `node_b` of way `way_id`, one column per metre.
    #[cfg(test)]
    pub(super) fn for_test(
        way_id: u64,
        node_a: u64,
        node_b: u64,
        cols: u32,
        rows: u32,
        tex: RgbaImage,
    ) -> Self {
        Self::test_wall(Owner::Way(way_id), node_a, node_b, cols, rows, tex)
    }

    /// Like `for_test`, for a wall of relation `relation_id`; its id is
    /// resolved when the relation is projected.
    #[cfg(test)]
    fn for_test_relation(
        relation_id: u64,
        node_a: u64,
        node_b: u64,
        cols: u32,
        rows: u32,
        tex: RgbaImage,
    ) -> Self {
        Self::test_wall(
            Owner::Relation(relation_id),
            node_a,
            node_b,
            cols,
            rows,
            tex,
        )
    }

    /// Like `for_test`, with the given cells (row-major, row 0 at the top)
    /// and no texture.
    #[cfg(test)]
    fn for_test_cells(
        way_id: u64,
        node_a: u64,
        node_b: u64,
        cols: u32,
        rows: u32,
        cells: Vec<(RGBTuple, Class)>,
    ) -> Self {
        Self::with_edges(
            Owner::Way(way_id),
            &[(node_a, node_b, 0.0, f64::from(cols))],
            0.0,
            cols,
            rows,
            cells,
            None,
        )
    }

    /// A wall of relation `relation_id` merged over several ring edges, each
    /// given as (node_a, node_b, s0, s1) with the metres from the start of
    /// this piece at its two nodes, the texture's column 0 starting `col0_m`
    /// metres in, the way the export describes a piece.
    #[cfg(test)]
    pub(super) fn for_test_relation_edges(
        relation_id: u64,
        edges: &[(u64, u64, f64, f64)],
        col0_m: f64,
        cols: u32,
        rows: u32,
        tex: Option<RgbaImage>,
    ) -> Self {
        Self::with_edges(
            Owner::Relation(relation_id),
            edges,
            col0_m,
            cols,
            rows,
            vec![((128, 128, 128), Class::Wall); (cols * rows) as usize],
            tex,
        )
    }

    #[cfg(test)]
    fn test_wall(
        owner: Owner,
        node_a: u64,
        node_b: u64,
        cols: u32,
        rows: u32,
        tex: RgbaImage,
    ) -> Self {
        Self::with_edges(
            owner,
            &[(node_a, node_b, 0.0, f64::from(cols))],
            0.0,
            cols,
            rows,
            vec![((128, 128, 128), Class::Wall); (cols * rows) as usize],
            Some(tex),
        )
    }

    #[cfg(test)]
    fn with_edges(
        owner: Owner,
        edges: &[(u64, u64, f64, f64)],
        col0_m: f64,
        cols: u32,
        rows: u32,
        cells: Vec<(RGBTuple, Class)>,
        tex: Option<RgbaImage>,
    ) -> Self {
        let bands = row_bands(&cells, cols, rows);
        FacadeWall {
            way_id: match owner {
                Owner::Way(id) => id,
                Owner::Relation(_) => UNPLACED,
            },
            owner,
            cols,
            rows,
            edges: edges
                .iter()
                .map(|&(node_a, node_b, s0, s1)| FacadeEdge {
                    node_a,
                    node_b,
                    col0: ((s0 - col0_m).floor() as i32).clamp(0, cols as i32),
                    col1: ((s1 - col0_m).ceil() as i32).clamp(0, cols as i32),
                    span_m: Some((s0, s1)),
                })
                .collect(),
            col0_m,
            cells,
            bands,
            tex,
        }
    }
}

/// What projecting the export onto the world grid produced.
struct Projection {
    /// World columns on exported walls.
    cells: FnvHashMap<(i32, i32), CellRef>,
    /// World columns per building, by the id it is built under.
    way_cells: FnvHashMap<u64, Vec<(i32, i32)>>,
    /// Per wall, its node A to node B direction in world blocks.
    wall_dir: Vec<(i32, i32)>,
    /// Per exported relation, the ring ids the generator builds it under.
    relation_rings: FnvHashMap<u64, Vec<u64>>,
}

/// A world column that lies on an exported wall.
#[derive(Clone, Copy, Debug)]
pub(super) struct CellRef {
    pub(super) wall: u32,
    pub(super) col: u16,
    /// Axis-snapped outward normal. Summed over a wall's cells it picks the
    /// side of the wall the building is not on, which is where its photo
    /// panel hangs (`displays.rs`).
    pub(super) nx: i8,
    pub(super) nz: i8,
}

/// Everything loaded from the export plus its projection onto the world grid.
pub struct FacadeStore {
    pub(super) walls: Vec<FacadeWall>,
    by_way: FnvHashMap<u64, Vec<usize>>,
    building_colour: FnvHashMap<u64, RGBTuple>,
    pub(super) cells: FnvHashMap<(i32, i32), CellRef>,
    /// World columns per way, so panel placement can walk one building.
    pub(super) way_cells: FnvHashMap<u64, Vec<(i32, i32)>>,
    /// Per wall, the vector from its node A to its node B in world blocks:
    /// the direction the texture's columns run, which decides whether a
    /// panel has to mirror its crop.
    pub(super) wall_dir: Vec<(i32, i32)>,
    /// The photos mode: the texture hangs as item display entities on top of
    /// the blocks (`displays.rs`).
    pub(super) displays: bool,
    /// Blocks per metre of this run; facade cells are one metre.
    pub(super) scale: f64,
}

/// Replaced on every install: the GUI generates several worlds in one process,
/// and each must see its own area's textures and mode (a `OnceLock` here once
/// made a run silently reuse the previous world's store).
static STORE: RwLock<Option<Arc<FacadeStore>>> = RwLock::new(None);

/// Whether [`STORE`] holds anything, written under its own write lock.
///
/// [`block_at`] and [`band_block_at`] are called from `apply_block_variety` for
/// every wall block of every building of every generation, facades or not, and
/// on a world without them the only thing `store` did was take a lock to hand
/// back `None`. Sixteen `rayon` tile threads sharing one `RwLock` word cost 69 ns
/// a call, measured on this machine under the lock on 2026-09-07 over 20 M calls
/// at 16 threads, against 0.2 ns for the relaxed load below: on a large world
/// that is about a second of contended cache line, spent by every user who never
/// turned the feature on.
///
/// Written while the write lock is held, so a reader that sees `true` and then
/// takes the read lock is ordered after the store it belongs to. A reader that
/// sees `false` returns `None`, which is either right or a race with an install,
/// and there is no install while a world is being built: `data_processing` calls
/// `install` or `clear` before the first tile thread starts.
static STORE_SET: AtomicBool = AtomicBool::new(false);

/// The one place [`STORE`] is written, so the flag beside it cannot drift.
fn set_store(value: Option<Arc<FacadeStore>>) {
    let mut guard = STORE.write().unwrap_or_else(|e| e.into_inner());
    STORE_SET.store(value.is_some(), Ordering::Release);
    *guard = value;
}

pub(super) fn store() -> Option<Arc<FacadeStore>> {
    if !STORE_SET.load(Ordering::Acquire) {
        return None;
    }
    STORE.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The panel placement tests of `displays.rs` share this store and their
/// registry, so they run one at a time. The building generator's facade tests
/// take it too, since a test that installs a store while another is reading
/// one sees the wrong walls.
#[cfg(test)]
pub(crate) static TEST_GLOBALS: Mutex<()> = Mutex::new(());

impl FacadeStore {
    /// Indexes the projected walls by the ids the generator asks with. A
    /// relation's colour is registered under every ring id it is built under.
    fn build(
        walls: Vec<FacadeWall>,
        colours: FnvHashMap<Owner, RGBTuple>,
        projection: Projection,
        displays: bool,
        scale: f64,
    ) -> Self {
        let mut by_way: FnvHashMap<u64, Vec<usize>> = FnvHashMap::default();
        for (i, w) in walls.iter().enumerate() {
            if w.way_id != UNPLACED {
                by_way.entry(w.way_id).or_default().push(i);
            }
        }
        let mut building_colour = FnvHashMap::default();
        for (owner, rgb) in colours {
            match owner {
                Owner::Way(id) => {
                    building_colour.insert(id, rgb);
                }
                Owner::Relation(id) => {
                    for ring_id in projection.relation_rings.get(&id).into_iter().flatten() {
                        building_colour.insert(*ring_id, rgb);
                    }
                }
            }
        }
        FacadeStore {
            walls,
            by_way,
            building_colour,
            cells: projection.cells,
            way_cells: projection.way_cells,
            wall_dir: projection.wall_dir,
            displays,
            scale,
        }
    }

    /// Whether the building built under `element_id` has at least one
    /// exported wall.
    fn has_building(&self, element_id: u64) -> bool {
        self.by_way.contains_key(&element_id)
    }

    /// The lab's whole-building colour of the building built under
    /// `element_id`.
    pub(super) fn building_colour(&self, element_id: u64) -> Option<RGBTuple> {
        self.building_colour.get(&element_id).copied()
    }

    /// Whether this run takes only the colours from the lab: below two
    /// blocks per metre a lab cell is at most one block, and the procedural
    /// windows read better than the lab's. From two up the full grid applies.
    fn colour_only(&self) -> bool {
        self.scale < 2.0
    }

    /// The exported wall under `(bx, bz)` for the building built under
    /// `element_id`, with the texture column there and the texture row of the
    /// generator's wall row `h` (the first wall block sits at
    /// `start_y_offset + 1`). None off the building's walls, below its first
    /// wall block or above the textured rows.
    fn wall_row_at(
        &self,
        bx: i32,
        h: i32,
        bz: i32,
        start_y_offset: i32,
        element_id: u64,
    ) -> Option<(&FacadeWall, u16, u32)> {
        let cell = self.cells.get(&(bx, bz))?;
        let wall = self.walls.get(cell.wall as usize)?;
        if wall.way_id != element_id {
            return None;
        }
        let height_index = h - start_y_offset - 1;
        if height_index < 0 {
            return None;
        }
        let metres_up = (height_index as f64 / self.scale.max(1e-9)).floor() as i64;
        let row = wall.rows as i64 - 1 - metres_up;
        if row < 0 {
            return None;
        }
        Some((wall, cell.col, row as u32))
    }

    /// Block for a wall position of the building built under `element_id`,
    /// if a facade cell of that building covers it; see the free `block_at`.
    /// Nothing below two blocks per metre, where only the colours apply.
    fn block_at(
        &self,
        bx: i32,
        h: i32,
        bz: i32,
        start_y_offset: i32,
        element_id: u64,
        window_block: Block,
    ) -> Option<Block> {
        if self.colour_only() {
            return None;
        }
        let (wall, col, row) = self.wall_row_at(bx, h, bz, start_y_offset, element_id)?;
        let (rgb, class) = wall.cell(u32::from(col), row)?;
        match class {
            Class::Wall => Some(facade_block_for_color(rgb)),
            Class::Window => Some(window_block),
            Class::Door => Some(DARK_OAK_PLANKS),
            Class::Unknown | Class::NoData => None,
        }
    }

    /// Block in the colour of the lab's floor band at a wall position of the
    /// building built under `element_id`, for the scales below two blocks per
    /// metre; see the free `band_block_at`.
    fn band_block_at(
        &self,
        bx: i32,
        h: i32,
        bz: i32,
        start_y_offset: i32,
        element_id: u64,
    ) -> Option<Block> {
        if !self.colour_only() {
            return None;
        }
        let (wall, _, row) = self.wall_row_at(bx, h, bz, start_y_offset, element_id)?;
        wall.band(row).map(facade_block_for_color)
    }

    /// Whether a photographed wall of the building built under `element_id`
    /// covers the world column `(bx, bz)`; see the free `photo_column`.
    fn photo_column(&self, bx: i32, bz: i32, element_id: u64) -> bool {
        self.cells
            .get(&(bx, bz))
            .and_then(|c| self.walls.get(c.wall as usize))
            .is_some_and(|w| w.way_id == element_id)
    }
}

/// Installs a hand-built store with the photo panels on, for the `displays.rs`
/// tests, which need walls on known cells without an export directory.
#[cfg(test)]
pub(super) fn install_displays_for_test(
    walls: Vec<FacadeWall>,
    cells: FnvHashMap<(i32, i32), CellRef>,
    wall_dir: Vec<(i32, i32)>,
    scale: f64,
) {
    install_for_test_mode(walls, cells, wall_dir, scale, true)
}

/// Like `install_displays_for_test` with the panels off: the walls apply as
/// blocks only, which is what the `blocks` mode does.
#[cfg(test)]
pub(super) fn install_blocks_for_test(
    walls: Vec<FacadeWall>,
    cells: FnvHashMap<(i32, i32), CellRef>,
    wall_dir: Vec<(i32, i32)>,
    scale: f64,
) {
    install_for_test_mode(walls, cells, wall_dir, scale, false)
}

#[cfg(test)]
fn install_for_test_mode(
    walls: Vec<FacadeWall>,
    cells: FnvHashMap<(i32, i32), CellRef>,
    wall_dir: Vec<(i32, i32)>,
    scale: f64,
    displays: bool,
) {
    let mut way_cells: FnvHashMap<u64, Vec<(i32, i32)>> = FnvHashMap::default();
    let mut ordered: Vec<(&(i32, i32), &CellRef)> = cells.iter().collect();
    ordered.sort_by_key(|(k, _)| **k);
    for (pos, cell) in ordered {
        way_cells
            .entry(walls[cell.wall as usize].way_id)
            .or_default()
            .push(*pos);
    }
    let projection = Projection {
        cells,
        way_cells,
        wall_dir,
        relation_rings: FnvHashMap::default(),
    };
    set_store(Some(Arc::new(FacadeStore::build(
        walls,
        FnvHashMap::default(),
        projection,
        displays,
        scale,
    ))));
}

/// Installs `walls` projected onto the buildings in `elements` the way
/// `install` does, with the photo panels on, for tests that need the real
/// ring mapping.
#[cfg(test)]
pub(super) fn install_display_elements_for_test(
    walls: Vec<FacadeWall>,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) {
    install_elements_for_test_mode(walls, elements, xzbbox, scale, true)
}

/// The colour every cell `install_wall_blocks_for_test` writes carries, so a
/// generator test can name the block those cells resolve to.
#[cfg(test)]
pub(crate) const TEST_WALL_RGB: RGBTuple = (128, 128, 128);

/// Installs a blocks-mode store for the building generator's tests: one wall
/// of plain `TEST_WALL_RGB` cells, `cols` metres by `rows` metres, over each
/// `(way_id, node_a, node_b, cols, rows)` ring edge given, projected onto
/// `elements` the way `install` does. The photo panels are off, so the walls
/// apply as blocks only, which is what a scale of 2 or more does.
#[cfg(test)]
pub(crate) fn install_wall_blocks_for_test(
    walls: &[(u64, u64, u64, u32, u32)],
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) {
    let walls = walls
        .iter()
        .map(|&(way_id, node_a, node_b, cols, rows)| {
            FacadeWall::for_test_cells(
                way_id,
                node_a,
                node_b,
                cols,
                rows,
                vec![(TEST_WALL_RGB, Class::Wall); (cols * rows) as usize],
            )
        })
        .collect();
    install_elements_for_test_mode(walls, elements, xzbbox, scale, false);
}

#[cfg(test)]
fn install_elements_for_test_mode(
    mut walls: Vec<FacadeWall>,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
    displays: bool,
) {
    let projection = project_cells(&mut walls, elements, xzbbox, scale);
    set_store(Some(Arc::new(FacadeStore::build(
        walls,
        FnvHashMap::default(),
        projection,
        displays,
        scale,
    ))));
}

/// Forgets any loaded textures, for runs without a facade directory.
pub fn clear() {
    set_store(None);
}

/// Whether the building built under `element_id` has at least one exported
/// wall. Ways are keyed by their way id, relation rings by the synthetic id
/// `generate_building_from_relation` builds them under.
pub fn has_building(element_id: u64) -> bool {
    store().is_some_and(|s| s.has_building(element_id))
}

/// The lab's whole-building colour, used as the base wall for walls without a
/// texture of their own. Keyed like `has_building`.
pub fn building_colour(element_id: u64) -> Option<RGBTuple> {
    store()?.building_colour(element_id)
}

/// Whether the photo panels are on for this run. Buildings with panels are
/// built as flat shells whatever the scale, since procedural ledges and window
/// frames would poke through the photograph.
pub fn displays_enabled() -> bool {
    store().is_some_and(|s| s.displays)
}

/// Whether a photographed wall of the building built under `element_id`
/// (keyed like `has_building`) covers the world column `(bx, bz)`, at any
/// height. False without a store.
///
/// This is the grain the plain shell is applied at. A building counts as
/// having a facade as soon as one of its walls was exported, and on the
/// Munich test box that is 111 of the 491 ring walls, so flattening the whole
/// shell cost the other 380 the windows and doors a normal Arnis building
/// would have. The column is the right unit rather than the ring segment
/// because it is a pure function of the position: the corner block two ring
/// segments share is decided the same way whichever segment is being drawn,
/// so a photographed wall meeting a normal one at a corner cannot leave a
/// gap or a doubled block there.
pub fn photo_column(bx: i32, bz: i32, element_id: u64) -> bool {
    store().is_some_and(|s| s.photo_column(bx, bz, element_id))
}

/// Whether the loaded export applies its colours only: the world is below two
/// blocks per metre, so a building with facade data keeps its procedural
/// style, with the lab's building colour as its wall block and the floor band
/// colours from `band_block_at`. False without a store.
pub fn colour_only() -> bool {
    store().is_some_and(|s| s.colour_only())
}

fn png_rgba(dir: &Path, name: &str) -> Option<RgbaImage> {
    // The name comes out of an export's own JSON, and `--mapillary-facades-dir`
    // will read a folder the user names, so it is not necessarily one we wrote.
    // A name is a file in the export, never a path: anything with a separator,
    // a parent segment or a drive letter would read outside the folder.
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        || name.contains("..")
    {
        return None;
    }
    // Flat export first, then the lab's run layout.
    for candidate in [
        dir.join(name),
        dir.join("texture").join("blocks").join(name),
        dir.join("texture").join("tex").join(name),
    ] {
        if candidate.exists() {
            return image::open(candidate).ok().map(|i| i.to_rgba8());
        }
    }
    None
}

fn as_u64(v: Option<&Value>) -> Option<u64> {
    v.and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
}

fn as_i32(v: Option<&Value>) -> Option<i32> {
    v.and_then(|x| {
        x.as_i64()
            .map(|i| i as i32)
            .or_else(|| x.as_f64().map(|f| f as i32))
    })
}

fn parse_wall(dir: &Path, w: &Value, owner: Owner) -> Option<FacadeWall> {
    let key = w.get("key")?.as_str()?.to_string();
    let cols = as_u64(w.get("cols"))? as u32;
    let rows = as_u64(w.get("rows"))? as u32;
    if cols == 0 || rows == 0 {
        return None;
    }
    let png = png_rgba(dir, w.get("png")?.as_str()?)?;
    if png.width() != cols || png.height() != rows {
        eprintln!(
            "Note: facade {key}: PNG is {}x{} but JSON says {cols}x{rows}; skipped",
            png.width(),
            png.height()
        );
        return None;
    }
    let cells: Vec<(RGBTuple, Class)> = png
        .pixels()
        .map(|p| ((p[0], p[1], p[2]), Class::from_alpha(p[3])))
        .collect();
    let bands = row_bands(&cells, cols, rows);

    // Where column 0 starts, in metres from the start of this piece.
    let col0_m = w
        .get("extent")
        .and_then(|e| e.get("s_l"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let mut edges = Vec::new();
    for e in w
        .get("edges")
        .and_then(|e| e.as_array())
        .into_iter()
        .flatten()
    {
        let (Some(a), Some(b)) = (as_u64(e.get("node_a")), as_u64(e.get("node_b"))) else {
            continue;
        };
        let span_m = match (
            e.get("s0").and_then(|v| v.as_f64()),
            e.get("s1").and_then(|v| v.as_f64()),
        ) {
            (Some(s0), Some(s1)) if s1 > s0 => Some((s0, s1)),
            _ => None,
        };
        edges.push(FacadeEdge {
            node_a: a,
            node_b: b,
            col0: as_i32(e.get("col0")).unwrap_or(0),
            col1: as_i32(e.get("col1")).unwrap_or(cols as i32 - 1),
            span_m,
        });
    }
    // A wall without an edge list is one edge from node_a to node_b over the
    // whole texture.
    if edges.is_empty() {
        edges.push(FacadeEdge {
            node_a: as_u64(w.get("node_a"))?,
            node_b: as_u64(w.get("node_b"))?,
            col0: 0,
            col1: cols as i32 - 1,
            span_m: None,
        });
    }

    let tex = w
        .get("tex")
        .and_then(|t| t.as_str())
        .and_then(|t| png_rgba(dir, t));

    Some(FacadeWall {
        way_id: match owner {
            Owner::Way(id) => id,
            Owner::Relation(_) => UNPLACED,
        },
        owner,
        cols,
        rows,
        edges,
        col0_m,
        cells,
        bands,
        tex,
    })
}

/// Per row, the median colour (per channel) of its wall cells; a row without
/// one takes the nearest row that has one, the lower row on a tie. Empty
/// when no row has a wall cell.
fn row_bands(cells: &[(RGBTuple, Class)], cols: u32, rows: u32) -> Vec<RGBTuple> {
    let own: Vec<Option<RGBTuple>> = (0..rows)
        .map(|row| {
            let mut channels: [Vec<u8>; 3] = Default::default();
            for col in 0..cols {
                if let Some(&((r, g, b), Class::Wall)) = cells.get((row * cols + col) as usize) {
                    channels[0].push(r);
                    channels[1].push(g);
                    channels[2].push(b);
                }
            }
            if channels[0].is_empty() {
                return None;
            }
            let [r, g, b] = channels.map(|mut c| {
                c.sort_unstable();
                c[c.len() / 2]
            });
            Some((r, g, b))
        })
        .collect();
    if own.iter().all(Option::is_none) {
        return Vec::new();
    }
    (0..rows as usize)
        .map(|row| {
            own.iter()
                .enumerate()
                .filter_map(|(r, band)| band.map(|rgb| (r, rgb)))
                .min_by_key(|(r, _)| ((*r as i64 - row as i64).abs(), std::cmp::Reverse(*r)))
                .map(|(_, rgb)| rgb)
                .expect("some row has a band")
        })
        .collect()
}

/// What one export directory holds.
struct Export {
    walls: Vec<FacadeWall>,
    colours: FnvHashMap<Owner, RGBTuple>,
    /// Mapillary ids of every image whose pixels reached one of `walls`, which
    /// is who the world owes its attribution to.
    images: BTreeSet<String>,
}

/// Reads every building JSON in `dir`. Walls below tier B are skipped. A
/// record of kind "relation" (key `r<id>`) belongs to the relation named by
/// its `relation_id`; every other record to the way named by its `way_id`.
fn load(dir: &Path) -> Result<Export, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    let mut walls = Vec::new();
    let mut colours = FnvHashMap::default();
    let mut images = BTreeSet::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json")
            || path.file_name().and_then(|n| n.to_str()) == Some("manifest.json")
        {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rec: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;

        // A relation record carries `way_id: null`, so each key is tried on
        // its own rather than falling through a JSON null.
        let owner = if rec.get("kind").and_then(|k| k.as_str()) == Some("relation") {
            match as_u64(rec.get("relation_id")).or_else(|| as_u64(rec.get("osm_id"))) {
                Some(id) => Owner::Relation(id),
                None => continue,
            }
        } else {
            match as_u64(rec.get("way_id")).or_else(|| as_u64(rec.get("osm_id"))) {
                Some(id) => Owner::Way(id),
                None => continue,
            }
        };
        if let Some(rgb) = rec
            .get("building_colour")
            .and_then(|c| c.get("rgb"))
            .and_then(|c| c.as_array())
        {
            if rgb.len() == 3 {
                let c = |i: usize| rgb[i].as_u64().unwrap_or(0).min(255) as u8;
                colours.insert(owner, (c(0), c(1), c(2)));
            }
        }
        for w in rec
            .get("walls")
            .and_then(|w| w.as_array())
            .into_iter()
            .flatten()
        {
            let tier = w.get("tier").and_then(|t| t.as_str()).unwrap_or("D");
            if !matches!(tier, "A" | "B") {
                continue;
            }
            if let Some(wall) = parse_wall(dir, w, owner) {
                // The images this wall was built from, so the world can credit
                // them. The lab writes an object per view and this crate's own
                // exporter a bare id, so both shapes are read.
                for v in w
                    .get("views")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                {
                    let id = v
                        .as_str()
                        .or_else(|| v.get("pano").and_then(|p| p.as_str()));
                    if let Some(id) = id.filter(|id| !id.is_empty()) {
                        images.insert(id.to_string());
                    }
                }
                walls.push(wall);
            }
        }
    }
    Ok(Export {
        walls,
        colours,
        images,
    })
}

/// Projects every exported wall onto the world grid for the buildings in
/// `elements`, walking the same Bresenham line the wall builder walks so the
/// cells line up exactly. A way's walls are projected onto its clipped node
/// ring under the way id. A relation's walls are projected onto the outer
/// rings the generator assembles from its member ways, each under the
/// synthetic id that ring is built under, and the wall takes that id as its
/// `way_id`; a wall lying on none of the rings stays unplaced.
fn project_cells(
    walls: &mut [FacadeWall],
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) -> Projection {
    let mut by_owner: FnvHashMap<Owner, Vec<usize>> = FnvHashMap::default();
    for (i, w) in walls.iter().enumerate() {
        by_owner.entry(w.owner).or_default().push(i);
    }
    let mut cells = FnvHashMap::default();
    let mut way_cells: FnvHashMap<u64, Vec<(i32, i32)>> = FnvHashMap::default();
    let mut wall_dir = vec![(0, 0); walls.len()];
    let mut relation_rings: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
    let world = Clip { scale };

    for element in elements {
        match element {
            ProcessedElement::Way(way) => {
                let Some(indices) = by_owner.get(&Owner::Way(way.id)) else {
                    continue;
                };
                let ring = Ring {
                    id: way.id,
                    nodes: &way.nodes,
                    world: &world,
                };
                for &wi in indices {
                    project_wall(
                        wi,
                        &walls[wi],
                        &ring,
                        &mut cells,
                        &mut way_cells,
                        &mut wall_dir[wi],
                    );
                }
            }
            ProcessedElement::Relation(relation) => {
                let Some(indices) = by_owner.get(&Owner::Relation(relation.id)) else {
                    continue;
                };
                for (ring_id, nodes) in relation_outer_rings(relation, xzbbox) {
                    relation_rings.entry(relation.id).or_default().push(ring_id);
                    let ring = Ring {
                        id: ring_id,
                        nodes: &nodes,
                        world: &world,
                    };
                    for &wi in indices {
                        let on_ring = project_wall(
                            wi,
                            &walls[wi],
                            &ring,
                            &mut cells,
                            &mut way_cells,
                            &mut wall_dir[wi],
                        );
                        if on_ring {
                            walls[wi].way_id = ring_id;
                        }
                    }
                }
            }
            ProcessedElement::Node(_) => {}
        }
    }
    Projection {
        cells,
        way_cells,
        wall_dir,
        relation_rings,
    }
}

/// What the projection needs to know about the world's edge.
struct Clip {
    /// Blocks per metre, the world scale. Only a wall the world edge cut needs
    /// it, to measure the surviving stub of the OSM edge in the metres the
    /// export's columns are numbered in.
    scale: f64,
}

impl Clip {
    /// Whether this ring vertex is one `clipping::clip_way_to_bbox` invented.
    ///
    /// The id says so outright: everything the clipper invents is numbered
    /// above `clipping::INVENTED_NODE_BASE`, clear of any id OSM can hand out,
    /// while a vertex that survived the clip keeps its own OSM node id
    /// (`assign_node_ids_preserving_endpoints`).
    ///
    /// Sitting on the world's edge is the weaker test and used to be this one:
    /// every invented vertex does sit there, but so does a real node that
    /// happens to, and that read as invented. Harmless, since the stub rule
    /// only runs on an edge that failed to match whole, but no longer needed.
    fn invented(&self, n: &ProcessedNode) -> bool {
        crate::clipping::is_invented_node_id(n.id)
    }
}

/// The ring one wall is being projected onto: the id the generator builds the
/// building under there, the ring's nodes, and where the world's edge is.
struct Ring<'a> {
    id: u64,
    nodes: &'a [ProcessedNode],
    world: &'a Clip,
}

/// Where one exported edge sits on the ring: which ring segment carries it,
/// and the wall metres at that segment's two ends.
struct Placement {
    /// Index of the ring segment, `nodes[seg]` to `nodes[seg + 1]`.
    seg: usize,
    /// Whether the segment runs the way the lab's columns do.
    forward: bool,
    /// Wall metres at the segment's start and end. `None` on an export that
    /// carries no metre span, which is projected by column interval instead.
    span: Option<(f64, f64)>,
}

/// Projects one wall onto the node ring of the building built under
/// `element_id`. True when at least one of the wall's edges lies on the ring.
///
/// Every block of a ring edge the wall covers gets the texture column under
/// it: column c covers metres [c, c + 1) from column 0, measured along the
/// wall the way the export does (`FacadeEdge::span_m`, `col0_m`). A block
/// before the texture or past its end is left alone: it belongs to the piece
/// the wall was exported in next to this one, to the wall that starts at that
/// corner, or to nobody. So a wall merged over several ring edges and
/// exported in pieces hands out each piece's columns once, in ring order,
/// and never stretches a piece over an edge it only partly covers.
///
/// An edge the world's edge cut in half is placed too, on the piece of it that
/// survived. `clipping::clip_way_to_bbox` keeps the id of every node inside the
/// world and invents one for each corner it creates, so a cut edge reaches here
/// as a segment with one real end and one invented one, and the surviving
/// metres are measured from the real end outwards. Without that, a ring that
/// only lost a corner kept none of the edges that were nowhere near it: on the
/// Munich test box 17 of the 42 exported buildings inside the world, 524 of
/// 1143 facade metres, got nothing at all.
fn project_wall(
    wi: usize,
    wall: &FacadeWall,
    ring: &Ring<'_>,
    cells: &mut FnvHashMap<(i32, i32), CellRef>,
    way_cells: &mut FnvHashMap<u64, Vec<(i32, i32)>>,
    wall_dir: &mut (i32, i32),
) -> bool {
    let (nodes, world, element_id) = (ring.nodes, ring.world, ring.id);
    let plan: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
    let clockwise = signed_area(&plan) > 0.0;
    let segments = nodes.len().saturating_sub(1);

    // Whole edges first, so a segment that carries one cannot also be taken
    // for the stub of another.
    let mut placed: Vec<Option<Placement>> = Vec::with_capacity(wall.edges.len());
    let mut taken = vec![false; segments];
    for edge in &wall.edges {
        let mut found = None;
        for i in 0..segments {
            let (na, nb) = (&nodes[i], &nodes[i + 1]);
            let forward = na.id == edge.node_a && nb.id == edge.node_b;
            let backward = na.id == edge.node_b && nb.id == edge.node_a;
            if !forward && !backward {
                continue;
            }
            taken[i] = true;
            found = Some(Placement {
                seg: i,
                forward,
                span: edge
                    .span_m
                    .map(|(s0, s1)| if forward { (s0, s1) } else { (s1, s0) }),
            });
            break;
        }
        placed.push(found);
    }

    for (edge, slot) in wall.edges.iter().zip(placed.iter_mut()) {
        if slot.is_some() {
            continue;
        }
        // An edge with no metre span cannot say where along itself the world
        // cut it, so only its whole self can be placed.
        let Some((s0, s1)) = edge.span_m else {
            continue;
        };
        // The cut edge is the one segment with a single real end, that end
        // being one of this edge's nodes and the other a corner the clipper
        // invented. Two such segments means the node lost both its neighbours
        // and nothing here can say which stub is which edge, so neither is
        // placed rather than one of them guessed. Segments running between the
        // same two blocks are the same stub drawn twice, which a ring that
        // doubles back on itself does, and count once.
        let mut candidate: Option<Placement> = None;
        let mut seen: Vec<((i32, i32), (i32, i32))> = Vec::new();
        for i in 0..segments {
            if taken[i] {
                continue;
            }
            let (na, nb) = (&nodes[i], &nodes[i + 1]);
            let a_on_edge = na.id == edge.node_a || na.id == edge.node_b;
            let b_on_edge = nb.id == edge.node_a || nb.id == edge.node_b;
            if a_on_edge == b_on_edge {
                continue;
            }
            let (real, invented) = if a_on_edge { (na, nb) } else { (nb, na) };
            if !world.invented(invented) {
                continue;
            }
            // Metres of the OSM edge this stub covers, from the end that
            // survived. Node positions are whole blocks, so this is only ever
            // as good as half a block, which is the resolution the columns are
            // handed out at anyway.
            let dx = f64::from(nb.x - na.x);
            let dz = f64::from(nb.z - na.z);
            let stub_m = (dx * dx + dz * dz).sqrt() / world.scale.max(1e-9);
            // Wall metres at the real end, then at the invented one, walking
            // the OSM edge away from the end that survived.
            let (from, to) = if real.id == edge.node_a {
                (s0, s0 + stub_m)
            } else {
                (s1, s1 - stub_m)
            };
            let span = if a_on_edge { (from, to) } else { (to, from) };
            let ends = ((na.x, na.z), (nb.x, nb.z));
            if seen.iter().any(|&(p, q)| (p, q) == ends || (q, p) == ends) {
                continue;
            }
            seen.push(ends);
            candidate = Some(Placement {
                seg: i,
                forward: span.1 >= span.0,
                span: Some(span),
            });
        }
        if seen.len() == 1 {
            if let Some(p) = candidate {
                taken[p.seg] = true;
                *slot = Some(p);
            }
        }
    }

    let mut on_ring = false;
    for (edge, placement) in wall.edges.iter().zip(placed.iter()) {
        let Some(placement) = placement else {
            continue;
        };
        let (na, nb) = (&nodes[placement.seg], &nodes[placement.seg + 1]);
        on_ring = true;
        // The lab's columns run from node A to node B; remember that
        // direction in world blocks for the painting crops.
        if placement.forward {
            wall_dir.0 += nb.x - na.x;
            wall_dir.1 += nb.z - na.z;
        } else {
            wall_dir.0 += na.x - nb.x;
            wall_dir.1 += na.z - nb.z;
        }
        let points = crate::bresenham::bresenham_line(na.x, 0, na.z, nb.x, 0, nb.z);
        let n = points.len();
        if n == 0 {
            continue;
        }
        // Outward normal of this edge in Arnis' frame (x east, z south),
        // the rule facade.rs uses, snapped to an axis for the photo panels.
        let (dx, dz) = ((nb.x - na.x) as f64, (nb.z - na.z) as f64);
        let len = (dx * dx + dz * dz).sqrt().max(1e-9);
        let (tx, tz) = (dx / len, dz / len);
        let (nx, nz) = if clockwise { (tz, -tx) } else { (-tz, tx) };
        let (snx, snz) = if nx.abs() >= nz.abs() {
            (nx.signum() as i8, 0)
        } else {
            (0, nz.signum() as i8)
        };

        let col_span = (edge.col1 - edge.col0).max(0);
        for (i_pt, (bx, _, bz)) in points.iter().enumerate() {
            // Fraction along the Arnis segment, from its start to its end.
            let f = if n > 1 {
                i_pt as f64 / (n - 1) as f64
            } else {
                0.0
            };
            let col = match placement.span {
                Some((from, to)) => {
                    // Metres from column 0 to this block; outside the
                    // texture the block is not this piece's.
                    let c = from + f * (to - from) - wall.col0_m;
                    if c < 0.0 || c >= f64::from(wall.cols) {
                        continue;
                    }
                    c.floor() as i32
                }
                // Exports without the metre span: the lab's column
                // interval stretched over the edge.
                None => {
                    let f = if placement.forward { f } else { 1.0 - f };
                    (edge.col0 + (f * col_span as f64).round() as i32)
                        .clamp(0, wall.cols as i32 - 1)
                }
            };
            cells.insert(
                (*bx, *bz),
                CellRef {
                    wall: wi as u32,
                    col: col as u16,
                    nx: snx,
                    nz: snz,
                },
            );
            way_cells.entry(element_id).or_default().push((*bx, *bz));
        }
    }
    on_ring
}

fn signed_area(points: &[(f64, f64)]) -> f64 {
    let mut acc = 0.0;
    for i in 0..points.len() {
        let (x0, z0) = points[i];
        let (x1, z1) = points[(i + 1) % points.len()];
        acc += x0 * z1 - x1 * z0;
    }
    acc
}

/// Credits every image an export folder says its walls were built from.
///
/// Mapillary imagery is CC BY-SA and the obligation travels with the pixels,
/// so a world built entirely out of a prepared folder owes exactly the same
/// attribution as one the pipeline fetched. What the folder can say varies:
/// an export this crate wrote lists each image's title and uploader in its
/// manifest, while the Python lab's export names only the image ids its walls
/// used. Where the uploader is not in the folder the credit still names the
/// photograph and links to it, which is where Mapillary shows who took it, and
/// says that is all the export carried.
fn record_folder_credits(dir: &Path, images: &BTreeSet<String>) {
    if images.is_empty() {
        return;
    }
    let named = manifest_credits(dir);
    let mut unnamed = 0usize;
    for id in images {
        let credit = named
            .get(id)
            .cloned()
            .unwrap_or_else(|| ImageCredit::by_id(id));
        if !credit.names_the_uploader() {
            unnamed += 1;
        }
        credits::record(credit);
    }
    // The GUI shows these under License and Credits; a CLI run has nowhere
    // else to put them, which is the same split `run_facade_pipeline` makes.
    if crate::progress::is_running_with_gui() {
        return;
    }
    println!("  Mapillary imagery used ({}):", images.len());
    for credit in credits::list() {
        println!("    {}", credit.line());
    }
    if unnamed > 0 {
        println!(
            "    {unnamed} of these name only the photograph: the facade folder carries the \
             image ids but not the uploader names. Each link opens the image, which names its \
             uploader."
        );
    }
}

/// The credits an export's manifest carries, by image id. Empty for an export
/// that has none, which is every one the Python lab wrote.
fn manifest_credits(dir: &Path) -> FnvHashMap<String, ImageCredit> {
    let mut out = FnvHashMap::default();
    let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
        return out;
    };
    let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    for c in manifest
        .get("credits")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        let Some(id) = c.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let field = |k: &str| {
            c.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        out.insert(
            id.to_string(),
            ImageCredit {
                id: id.to_string(),
                title: match field("title") {
                    t if t.is_empty() => id.to_string(),
                    t => t,
                },
                username: field("creator"),
                // The manifest carries the username, which is the stable
                // public route to a profile; there is no numeric id in it.
                user_id: String::new(),
            },
        );
    }
    out
}

/// Loads the export and projects it onto this run's buildings. `xzbbox` is
/// the world's, which the generator clips relation rings to. Prints a
/// one-line summary; a bad directory is a warning, never a failed run.
pub fn install(dir: &Path, elements: &[ProcessedElement], args: &Args, xzbbox: &XZBBox) {
    let Export {
        mut walls,
        colours,
        images,
    } = match load(dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Warning: facade textures not loaded: {e}");
            clear();
            return;
        }
    };
    let loaded_buildings = walls
        .iter()
        .map(|w| w.owner)
        .collect::<FnvHashSet<Owner>>()
        .len();
    let projection = project_cells(&mut walls, elements, xzbbox, args.scale);
    let matched = projection.way_cells.len();
    let columns = projection.cells.len();
    // A folder given on the command line never went through the pipeline, so
    // nothing has credited its imagery yet, and the licence is on the pixels
    // either way. After the projection, because a folder drawn over another
    // bbox puts no photograph into this world and owes it no attribution.
    if args.mapillary_facades_dir.is_some() && matched > 0 {
        record_folder_credits(dir, &images);
    }
    // Each of these says the same thing twice: a short line for the status
    // line, which is one line of a narrow panel and wraps a sentence across the
    // progress bar, and the whole of it for the terminal, which has the width
    // for the path or the count behind it and keeps it after the run.
    if walls.is_empty() {
        eprintln!(
            "Warning: Facade textures: nothing usable in {} (no tier A/B walls). \
             Run export_arnis.py first.",
            dir.display()
        );
        crate::progress::emit_gui_progress_update(
            crate::progress::MESSAGE_ONLY,
            "Facades: nothing usable in that export",
        );
        clear();
        return;
    }
    if matched == 0 {
        eprintln!(
            "Warning: Facade textures: {} walls loaded but none of their buildings are in this \
             area. The export covers a different bbox.",
            walls.len()
        );
        crate::progress::emit_gui_progress_update(
            crate::progress::MESSAGE_ONLY,
            "Facades: that export covers a different area",
        );
    } else {
        // MESSAGE_ONLY, like the two branches above: a real fraction here sends
        // the GUI bar back to 0 per cent in the middle of a generation.
        crate::progress::emit_gui_progress_update(
            crate::progress::MESSAGE_ONLY,
            &format!("Facades: textures for {matched} buildings"),
        );
    }
    // The photo panels are Java entities; other formats build the blocks only.
    let java = !(args.bedrock || args.luanti);
    let displays = args.mapillary_facade_mode.places_displays() && java;
    println!(
        "  Facade textures: {} walls on {loaded_buildings} buildings loaded, {matched} buildings matched in this area, {columns} wall columns{}{}",
        walls.len(),
        if displays {
            format!(", photo panels at {} px per block", args.facade_px)
        } else {
            String::new()
        },
        if args.scale < 2.0 {
            ", colours only (world below 2 blocks per metre)"
        } else {
            ""
        }
    );
    set_store(Some(Arc::new(FacadeStore::build(
        walls, colours, projection, displays, args.scale,
    ))));
}

/// Block for a wall position, if a facade cell of the building built under
/// `element_id` covers it (keyed like `has_building`).
///
/// `h` is the generator's wall row (the first wall block sits at
/// `start_y_offset + 1`). Cells classed unknown or empty return `None` so the
/// caller keeps whatever it would have placed. Always `None` below two
/// blocks per metre, where only the colours apply (see `band_block_at`).
pub fn block_at(
    bx: i32,
    h: i32,
    bz: i32,
    start_y_offset: i32,
    element_id: u64,
    window_block: Block,
) -> Option<Block> {
    store()?.block_at(bx, h, bz, start_y_offset, element_id, window_block)
}

/// Block in the colour of the lab's floor band at a wall position of the
/// building built under `element_id` (keyed like `has_building`), for the
/// scales below two blocks per metre, where only the colours apply.
///
/// `h` is the generator's wall row as in `block_at`. The band of a texture
/// row is the median colour of its wall cells, or the nearest row's when it
/// has none; the lab's windows and doors are ignored here. None above the
/// textured rows, below the first wall block, off the building's walls, and
/// from two blocks per metre up.
pub fn band_block_at(
    bx: i32,
    h: i32,
    bz: i32,
    start_y_offset: i32,
    element_id: u64,
) -> Option<Block> {
    store()?.band_block_at(bx, h, bz, start_y_offset, element_id)
}

/// Clearance between a preview quad and the extruded footprint it hangs on.
///
/// The preview draws both without a depth bias and the footprint is a coarse
/// extrusion, so a quad closer than this flickers against the wall as the
/// camera moves. A metre is invisible at preview zoom and stops the fight.
const PREVIEW_OUTWARD_M: f64 = 1.0;

/// How many walls the preview hands the webview.
///
/// Each carries its own texture as a base64 data URL, so an area with a
/// thousand built walls would otherwise cost tens of megabytes of JSON to draw
/// quads a few pixels wide.
const PREVIEW_MAX_WALLS: usize = 200;

/// A wall the preview could draw, before its texture has been read.
///
/// Only the best [`PREVIEW_MAX_WALLS`] survive the ranking, and the texture is
/// the expensive part of one, so candidates are gathered and cut first and the
/// PNGs are read afterwards.
struct PreviewWall {
    key: String,
    confidence: f64,
    /// The two ground corners in lon/lat, already pushed clear of the footprint.
    a: [f64; 2],
    b: [f64; 2],
    height_m: f64,
    tex: std::path::PathBuf,
}

/// Walls of the facade cache as JSON for the 3D preview.
///
/// The cache and not a folder the user names: the pipeline writes its export
/// under `facades/e<epoch>-<digest>/exports/`, so whatever a generation or the
/// Precompute button has already built for this area is there to be shown, and
/// the preview needs neither a setting nor the network to find it.
pub fn preview_walls_from_cache(bbox_text: &str) -> Result<String, String> {
    let exports = super::cache::exports_root(&super::facade_cache_dir());
    preview_walls_json(&exports, bbox_text)
}

/// Every export under `exports` as one preview feature list.
///
/// Each entry carries the wall's two ground corners in lon/lat, pushed
/// [`PREVIEW_OUTWARD_M`] out along the outward normal, the wall height in
/// metres and the 8 px/m texture as a PNG data URL. Best confidence first.
///
/// The exports are read newest first and the first record of a wall wins, so a
/// re-run of an area supersedes the run before it rather than being drawn on
/// top of it. Walls are filtered by the box one at a time, so an export of
/// somewhere else costs a JSON parse and contributes nothing.
pub fn preview_walls_json(exports: &Path, bbox_text: &str) -> Result<String, String> {
    use base64::Engine as _;

    let bbox = crate::coordinate_system::geographic::LLBBox::from_str(bbox_text)?;
    let mut seen: FnvHashSet<String> = FnvHashSet::default();
    let mut walls: Vec<PreviewWall> = Vec::new();
    for dir in exports_newest_first(exports) {
        collect_preview_walls(&dir, &bbox, &mut seen, &mut walls);
    }

    walls.sort_by(|x, y| y.confidence.total_cmp(&x.confidence));
    walls.truncate(PREVIEW_MAX_WALLS);

    let out: Vec<serde_json::Value> = walls
        .into_iter()
        .filter_map(|w| {
            // A texture that has gone missing since the record was read drops
            // the wall rather than the whole preview: an export half swept out
            // from under us must not blank the ones beside it.
            let bytes = std::fs::read(&w.tex).ok()?;
            Some(serde_json::json!({
                "key": w.key,
                "a": w.a,
                "b": w.b,
                "h": w.height_m,
                "conf": w.confidence,
                "tex": format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&bytes)
                ),
            }))
        })
        .collect();
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

/// The export directories under `exports`, newest first.
///
/// Newest first is what makes the merge deterministic: the same wall built
/// twice is shown as the later run built it. A directory whose modified time
/// cannot be read sorts oldest, because the only way that happens is a
/// directory being swept while this walks the tree.
fn exports_newest_first(exports: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(exports) else {
        return Vec::new();
    };
    let mut dirs: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let when = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (when, e.path())
        })
        .collect();
    // The path breaks ties, so two exports written in the same file system tick
    // do not swap places between two calls and make the preview flicker.
    dirs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    dirs.into_iter().map(|(_, path)| path).collect()
}

/// Adds one export's tier A and B walls inside the box to `walls`.
///
/// `seen` carries across exports and holds the wall keys already taken, so an
/// older export cannot displace a newer one's record of the same wall.
fn collect_preview_walls(
    dir: &Path,
    bbox: &crate::coordinate_system::geographic::LLBBox,
    seen: &mut FnvHashSet<String>,
    walls: &mut Vec<PreviewWall>,
) {
    // A little beyond the box so walls on its edge still show.
    let margin_lat = 0.0005;
    let margin_lon = 0.0008;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json")
            || path.file_name().and_then(|n| n.to_str()) == Some("manifest.json")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        // Ring winding decides which side is outside; the export's rings are CCW
        // in ENU (x east, y north), where the outward normal of a->b is (t_y, -t_x).
        let ring: Vec<(f64, f64)> = rec
            .get("ring_lonlat")
            .and_then(|r| r.as_array())
            .map(|r| {
                r.iter()
                    .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default();
        let lat0 = ring.first().map(|p| p.1).unwrap_or(bbox.min().lat());
        let cos_lat = lat0.to_radians().cos().max(1e-6);
        let area = {
            let mut acc = 0.0;
            for i in 0..ring.len() {
                let (x0, y0) = (ring[i].0 * cos_lat, ring[i].1);
                let (x1, y1) = (
                    ring[(i + 1) % ring.len()].0 * cos_lat,
                    ring[(i + 1) % ring.len()].1,
                );
                acc += x0 * y1 - x1 * y0;
            }
            acc
        };
        let ccw = area > 0.0;

        for w in rec
            .get("walls")
            .and_then(|w| w.as_array())
            .into_iter()
            .flatten()
        {
            if !matches!(
                w.get("tier").and_then(|t| t.as_str()),
                Some("A") | Some("B")
            ) {
                continue;
            }
            let (Some(a), Some(b)) = (w.get("a_lonlat"), w.get("b_lonlat")) else {
                continue;
            };
            let (Some(ax), Some(ay), Some(bx), Some(by)) = (
                a.get(0).and_then(|v| v.as_f64()),
                a.get(1).and_then(|v| v.as_f64()),
                b.get(0).and_then(|v| v.as_f64()),
                b.get(1).and_then(|v| v.as_f64()),
            ) else {
                continue;
            };
            let mid_lat = (ay + by) / 2.0;
            let mid_lon = (ax + bx) / 2.0;
            if mid_lat < bbox.min().lat() - margin_lat
                || mid_lat > bbox.max().lat() + margin_lat
                || mid_lon < bbox.min().lng() - margin_lon
                || mid_lon > bbox.max().lng() + margin_lon
            {
                continue;
            }
            let Some(tex_name) = w.get("tex").and_then(|t| t.as_str()) else {
                continue;
            };
            // A bare file name beside the record, nothing else: the export is
            // read from the cache, which anything on the machine can write,
            // and a path here would be read from wherever it points.
            if Path::new(tex_name).file_name().and_then(|f| f.to_str()) != Some(tex_name) {
                continue;
            }
            let key = w.get("key").and_then(|k| k.as_str()).unwrap_or("");
            // An unkeyed record cannot be deduplicated, so it is dropped rather
            // than drawn once per export that carries it.
            if key.is_empty() || !seen.insert(key.to_string()) {
                continue;
            }
            // Tangent in local metres, outward normal, back to degrees.
            let m_per_deg_lat = 111_195.0;
            let m_per_deg_lon = m_per_deg_lat * cos_lat;
            let (tx, ty) = ((bx - ax) * m_per_deg_lon, (by - ay) * m_per_deg_lat);
            let len = (tx * tx + ty * ty).sqrt().max(1e-6);
            let (tx, ty) = (tx / len, ty / len);
            let (nx, ny) = if ccw { (ty, -tx) } else { (-ty, tx) };
            let (dlon, dlat) = (
                nx * PREVIEW_OUTWARD_M / m_per_deg_lon,
                ny * PREVIEW_OUTWARD_M / m_per_deg_lat,
            );
            let height_m = w
                .get("height_used_m")
                .and_then(|h| h.as_f64())
                .or_else(|| w.get("rows").and_then(|r| r.as_f64()))
                .unwrap_or(9.0);
            walls.push(PreviewWall {
                key: key.to_string(),
                confidence: w.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0),
                a: [ax + dlon, ay + dlat],
                b: [bx + dlon, by + dlat],
                height_m,
                tex: dir.join(tex_name),
            });
        }
    }
}

/// The building relation r147094 of the Munich export
/// (`tools/facade_lab/out/munich_ref`), for the tests here and in
/// `displays.rs`: a retail block whose south wall the lab merged over five
/// ring edges and exported in three pieces.
#[cfg(test)]
pub(super) mod test_fixtures {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::osm_parser::{
        ProcessedMember, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
    };

    /// The relation's two member ways.
    pub(crate) const MEMBER_WAYS: [u64; 2] = [35031257, 35031259];

    /// The outer ring's nodes in the export's order, laid out as an
    /// axis-aligned outline so that every ring block is exactly one metre:
    /// the five edges of the merged south wall run east along z = 60 with
    /// their lengths rounded to whole metres (6, 24, 9, 14 and 12 m), the
    /// other eleven edges close the ring to the north. The real outline is
    /// this shape turned by 45 degrees, which would put a Bresenham step
    /// between every two blocks.
    pub(crate) const RING: [(u64, i32, i32); 16] = [
        (410874364, 85, 60),
        (1121737041, 85, 50),
        (410874369, 85, 40),
        (410874365, 75, 40),
        (2373874254, 65, 40),
        (2373874262, 55, 40),
        (2515092929, 45, 40),
        (410874361, 35, 40),
        (2515092926, 25, 40),
        (21486943, 20, 40),
        (2950486613, 20, 50),
        (21486944, 20, 60),
        (2545319959, 26, 60),
        (2545319965, 50, 60),
        (1121737122, 59, 60),
        (2545319977, 73, 60),
    ];

    /// The five OSM edges of wall 8, with the metres along the wall at their
    /// nodes as the export measured them (the wall is 65.19 m long).
    pub(crate) const WALL8_EDGES: [(u64, u64, f64, f64); 5] = [
        (21486944, 2545319959, 0.0, 5.98236255448797),
        (2545319959, 2545319965, 5.98236255448797, 29.69858583772065),
        (2545319965, 1121737122, 29.69858583772065, 39.14598322043684),
        (
            1121737122,
            2545319977,
            39.14598322043684,
            53.513133202764855,
        ),
        (2545319977, 410874364, 53.513133202764855, 65.19037588743956),
    ];

    /// The three pieces wall 8 was exported in: metres from node A to the
    /// piece's start, the piece's `extent.s_l` and its column count.
    pub(crate) const WALL8_PIECES: [(f64, f64, u32); 3] = [
        (0.0, 2.4365994606811867, 17),
        (21.73012529581318, -2.069101707821279, 23),
        (43.46025059162636, -0.26757924940010724, 21),
    ];

    /// The relation as the parser hands it over: the first member way runs
    /// from node 410874364 round to node 21486944, the second one holds the
    /// south wall back to 410874364, sharing the two end nodes.
    pub(crate) fn r147094_relation() -> ProcessedRelation {
        let node = |&(id, x, z): &(u64, i32, i32)| ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        };
        let member = |way_id: u64, nodes: Vec<ProcessedNode>| ProcessedMember {
            role: ProcessedMemberRole::Outer,
            way: Arc::new(ProcessedWay {
                id: way_id,
                tags: HashMap::new(),
                nodes,
            }),
        };
        let north: Vec<ProcessedNode> = RING[..=11].iter().map(node).collect();
        let south: Vec<ProcessedNode> = RING[11..].iter().chain(&RING[..1]).map(node).collect();
        ProcessedRelation {
            id: 147094,
            tags: HashMap::from([
                ("building".to_string(), "retail".to_string()),
                ("type".to_string(), "multipolygon".to_string()),
                ("building:levels".to_string(), "4".to_string()),
            ]),
            members: vec![member(MEMBER_WAYS[0], north), member(MEMBER_WAYS[1], south)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::GLASS;
    use crate::element_processing::buildings::relation_ring_id;
    use crate::osm_parser::{
        ProcessedMember, ProcessedMemberRole, ProcessedRelation, ProcessedWay,
    };
    use std::collections::HashMap;

    /// The fast path past the store's lock must agree with the store itself.
    ///
    /// `store` returns `None` on the flag alone, so a flag left behind by an
    /// install would hand every wall block of a facade world its texture back as
    /// `None`, silently, and a flag left set by `clear` would only cost a lock.
    /// The first is facade loss with nothing to see, so both directions are
    /// pinned here rather than left to the placement tests.
    #[test]
    fn the_store_gate_never_says_empty_while_a_store_is_installed() {
        let _guard = TEST_GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 64, 64).unwrap();

        clear();
        assert!(!STORE_SET.load(Ordering::Acquire));
        assert!(store().is_none());

        install_elements_for_test_mode(Vec::new(), &[], &xzbbox, 2.0, false);
        assert!(STORE_SET.load(Ordering::Acquire));
        assert!(
            store().is_some(),
            "the gate hid a store the generator would have used"
        );

        clear();
        assert!(!STORE_SET.load(Ordering::Acquire));
        assert!(store().is_none());
    }

    // ---------------------------------------------------- the 3D preview

    /// One export directory holding one building with one wall, written the
    /// way `pipeline::write_export` writes them.
    ///
    /// The ring is a square around the Munich test box's south-west corner,
    /// wound counter-clockwise, and the wall is its south edge.
    fn write_preview_export(
        exports: &Path,
        name: &str,
        at: (f64, f64),
        wall: (&str, &str, f64, f64),
    ) {
        let (lon, lat) = at;
        let (wall_key, tier, confidence, height_m) = wall;
        let dir = exports.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let d = 0.0002;
        let record = serde_json::json!({
            "key": "w1",
            "ring_lonlat": [
                [lon - d, lat - d], [lon + d, lat - d], [lon + d, lat + d], [lon - d, lat + d],
            ],
            "walls": [{
                "key": wall_key,
                "tier": tier,
                "confidence": confidence,
                "height_used_m": height_m,
                "a_lonlat": [lon - d, lat - d],
                "b_lonlat": [lon + d, lat - d],
                "tex": format!("{wall_key}_tex.png"),
            }],
        });
        std::fs::write(dir.join("w1.json"), serde_json::to_vec(&record).unwrap()).unwrap();
        // A one pixel PNG: the preview only base64s the bytes, it never
        // decodes them.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(dir.join(format!("{wall_key}_tex.png")), &png).unwrap();
        // The two exports of one area must not share a modified time, or
        // "newest first" has nothing to sort on.
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    const PREVIEW_BOX: &str = "48.135635,11.578243,48.137225,11.580818";

    #[test]
    fn the_preview_is_empty_when_nothing_has_been_built_for_the_area() {
        let tmp = tempfile::tempdir().unwrap();
        // No exports directory at all, which is every user before their first
        // generation with the feature on.
        let json = preview_walls_json(&tmp.path().join("exports"), PREVIEW_BOX).unwrap();
        assert_eq!(json, "[]");
    }

    /// The cache accumulates one export per run, so the preview has to merge
    /// them and let the later run win rather than drawing a wall twice.
    #[test]
    fn the_preview_takes_the_newest_export_of_a_wall() {
        let tmp = tempfile::tempdir().unwrap();
        let exports = tmp.path().join("exports");
        let (lon, lat) = (11.5795, 48.1365);
        write_preview_export(&exports, "aaaa-1", (lon, lat), ("w1_0", "A", 0.9, 8.0));
        write_preview_export(&exports, "aaaa-2", (lon, lat), ("w1_0", "A", 0.9, 12.0));

        let walls: Vec<Value> =
            serde_json::from_str(&preview_walls_json(&exports, PREVIEW_BOX).unwrap()).unwrap();
        assert_eq!(walls.len(), 1, "one wall, not one per export");
        assert_eq!(walls[0]["h"].as_f64(), Some(12.0), "the later run wins");
        assert!(walls[0]["tex"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    /// Two areas precomputed at different times both stay in the cache, and
    /// the preview shows whichever of them the current box is over.
    #[test]
    fn the_preview_shows_the_area_the_box_is_over_whichever_run_built_it() {
        let tmp = tempfile::tempdir().unwrap();
        let exports = tmp.path().join("exports");
        // Inside the box, and a whole degree north of it.
        write_preview_export(
            &exports,
            "aaaa-1",
            (11.5795, 48.1365),
            ("here_0", "A", 0.9, 8.0),
        );
        write_preview_export(
            &exports,
            "bbbb-1",
            (11.5795, 49.1365),
            ("far_0", "A", 0.9, 8.0),
        );

        let walls: Vec<Value> =
            serde_json::from_str(&preview_walls_json(&exports, PREVIEW_BOX).unwrap()).unwrap();
        assert_eq!(walls.len(), 1);
        assert_eq!(walls[0]["key"].as_str(), Some("here_0"));
    }

    /// The preview is a check on what the world will get, and tiers C and D
    /// are not built as facades, so drawing them would be a lie.
    #[test]
    fn the_preview_leaves_out_the_walls_no_world_would_build() {
        let tmp = tempfile::tempdir().unwrap();
        let exports = tmp.path().join("exports");
        let (lon, lat) = (11.5795, 48.1365);
        write_preview_export(&exports, "aaaa-1", (lon, lat), ("good_0", "B", 0.7, 8.0));
        write_preview_export(&exports, "aaaa-2", (lon, lat), ("weak_0", "C", 0.5, 8.0));

        let walls: Vec<Value> =
            serde_json::from_str(&preview_walls_json(&exports, PREVIEW_BOX).unwrap()).unwrap();
        assert_eq!(walls.len(), 1);
        assert_eq!(walls[0]["key"].as_str(), Some("good_0"));
    }

    #[test]
    fn classes_decode_from_alpha() {
        assert_eq!(Class::from_alpha(255), Class::Wall);
        assert_eq!(Class::from_alpha(192), Class::Window);
        assert_eq!(Class::from_alpha(128), Class::Door);
        assert_eq!(Class::from_alpha(64), Class::Unknown);
        assert_eq!(Class::from_alpha(0), Class::NoData);
        assert_eq!(Class::from_alpha(7), Class::NoData);
    }

    #[test]
    fn normals_point_out_of_a_square_either_winding() {
        // The same rule facade.rs tests; a square in Arnis' frame, both windings.
        let square = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(signed_area(&square) > 0.0);
        let mut reversed = square.clone();
        reversed.reverse();
        assert!(signed_area(&reversed) < 0.0);
    }

    #[test]
    fn nothing_is_applied_without_a_store() {
        assert!(!has_building(1));
        assert!(block_at(0, 5, 0, 0, 1, DARK_OAK_PLANKS).is_none());
    }

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn member(way_id: u64, tags: &[(&str, &str)], nodes: Vec<ProcessedNode>) -> ProcessedMember {
        ProcessedMember {
            role: ProcessedMemberRole::Outer,
            way: Arc::new(ProcessedWay {
                id: way_id,
                tags: tags
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                nodes,
            }),
        }
    }

    fn grey_tex() -> RgbaImage {
        RgbaImage::from_pixel(8, 8, image::Rgba([128, 128, 128, 255]))
    }

    /// Writes one building of a facade export into `dir`: a single tier A
    /// wall, its block PNG, and `views` verbatim, so a case can hand it either
    /// export's shape.
    fn write_export_building(dir: &Path, way_id: u64, views: Value) {
        let key = format!("w{way_id}_0");
        RgbaImage::from_pixel(4, 3, image::Rgba([128, 128, 128, 255]))
            .save(dir.join(format!("{key}.png")))
            .unwrap();
        let record = serde_json::json!({
            "key": format!("w{way_id}"),
            "kind": "way",
            "way_id": way_id,
            "walls": [{
                "key": key,
                "tier": "A",
                "cols": 4,
                "rows": 3,
                "png": format!("{key}.png"),
                "node_a": 1,
                "node_b": 2,
                "views": views,
            }],
        });
        std::fs::write(
            dir.join(format!("w{way_id}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }

    /// A folder given by hand carries no pipeline run behind it, so the images
    /// its walls name are the only record of what the world owes attribution
    /// for. Both export shapes are read: the lab writes an object per view,
    /// this crate's own exporter a bare id.
    #[test]
    fn an_export_folder_names_the_images_its_walls_were_built_from() {
        let tmp = tempfile::tempdir().unwrap();
        write_export_building(
            tmp.path(),
            55,
            serde_json::json!([{"pano": "111"}, {"pano": "222"}]),
        );
        write_export_building(tmp.path(), 56, serde_json::json!(["222", "333"]));

        let export = load(tmp.path()).unwrap();
        assert_eq!(export.walls.len(), 2);
        assert_eq!(
            export.images.iter().cloned().collect::<Vec<_>>(),
            vec!["111".to_string(), "222".to_string(), "333".to_string()],
            "an image two walls used is owed one credit"
        );
    }

    /// What the credits say depends on what the folder carried: an export this
    /// crate wrote names the uploaders in its manifest, the lab's names only
    /// the images, and a world built out of either still has to credit the
    /// pixels it is made of.
    #[test]
    fn a_folder_credits_its_imagery_whether_or_not_it_names_the_uploaders() {
        let _guard = TEST_GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let images: BTreeSet<String> = ["111", "222"].iter().map(|s| s.to_string()).collect();

        // The lab's export: a manifest with no credits in it at all.
        let lab = tempfile::tempdir().unwrap();
        std::fs::write(lab.path().join("manifest.json"), b"{\"run\": \"munich\"}").unwrap();
        credits::reset();
        record_folder_credits(lab.path(), &images);
        let listed = credits::list();
        assert_eq!(listed.len(), 2);
        assert!(!listed[0].names_the_uploader());
        assert!(
            listed[0].line().contains("pKey=111") && listed[0].line().contains("does not name"),
            "{}",
            listed[0].line()
        );

        // This crate's export: the manifest carries the attribution the run
        // fetched, so the same folder credits by name.
        let ours = tempfile::tempdir().unwrap();
        std::fs::write(
            ours.path().join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "credits": [
                    {"id": "111", "title": "Ledererstrasse, Munich", "creator": "nunocaldeira"},
                    {"id": "999", "title": "elsewhere", "creator": "someone"},
                ],
            }))
            .unwrap(),
        )
        .unwrap();
        credits::reset();
        record_folder_credits(ours.path(), &images);
        let listed = credits::list();
        assert_eq!(
            listed.len(),
            2,
            "only the images the walls used are credited"
        );
        assert_eq!(listed[0].username, "nunocaldeira");
        assert_eq!(
            listed[0].profile_url(),
            "https://www.mapillary.com/app/user/nunocaldeira"
        );
        assert!(
            !listed[1].names_the_uploader(),
            "222 is not in the manifest"
        );
        credits::reset();
    }

    /// The defect this guards: a building the world's edge cut used to lose
    /// every one of its facades, including the walls nowhere near the cut,
    /// because `clip_way_to_bbox` renamed the whole ring and `project_wall`
    /// asks for an edge by its two node ids. On the Munich test box that was
    /// 17 of the 42 exported buildings inside the world and 524 of 1143
    /// facade metres, silently, on every generation.
    #[test]
    fn a_ring_the_world_edge_cut_keeps_the_walls_that_survived() {
        // A 30 x 15 building whose eastern third lies outside a 30 x 30
        // world, so the ring loses both its eastern corners.
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 30, 30).unwrap();
        let full = vec![
            node(1, 10, 10),
            node(2, 40, 10),
            node(3, 40, 25),
            node(4, 10, 25),
            node(1, 10, 10),
        ];
        let clipped = crate::clipping::clip_way_to_bbox(&full, &xzbbox);
        assert!(
            clipped.iter().any(|n| n.id == 1) && clipped.iter().any(|n| n.id == 4),
            "the two nodes inside the world keep their ids: {:?}",
            clipped.iter().map(|n| n.id).collect::<Vec<_>>()
        );
        assert!(
            !clipped.iter().any(|n| n.id == 2 || n.id == 3),
            "the two nodes outside are gone"
        );

        let way = ProcessedWay {
            id: 55,
            tags: HashMap::from([("building".to_string(), "yes".to_string())]),
            nodes: clipped,
        };
        let elements = vec![ProcessedElement::Way(way)];
        // Wall 0 is the western edge, 15 m from node 4 to node 1, nowhere near
        // the cut. Wall 1 is the northern edge, 30 m from node 1 to node 2, of
        // which the world holds the first 20. Wall 2 is the southern edge, 30 m
        // from node 3 to node 4, cut the other way round: the world holds its
        // last 20 m, so its surviving columns are the far ones.
        let mut walls = vec![
            FacadeWall::for_test(55, 4, 1, 15, 6, grey_tex()),
            FacadeWall::for_test(55, 1, 2, 30, 6, grey_tex()),
            FacadeWall::for_test(55, 3, 4, 30, 6, grey_tex()),
        ];
        let projection = project_cells(&mut walls, &elements, &xzbbox, 1.0);

        // The whole western wall: column 0 at node 4, one block per metre,
        // the block at node 1 belonging to the wall that starts there.
        let west: Vec<(i32, i32)> = (0..15).map(|c| (10, 25 - c)).collect();
        for (c, pos) in west.iter().enumerate() {
            let cell = projection.cells.get(pos).unwrap_or_else(|| {
                panic!("the western wall lost its block at {pos:?} (column {c})")
            });
            assert_eq!((cell.wall, cell.col), (0, c as u16));
            assert_eq!((cell.nx, cell.nz), (-1, 0), "the west wall faces west");
        }

        // The northern wall keeps the 21 blocks the world holds, columns 0 to
        // 20 measured from node 1, and claims nothing past the world edge.
        for c in 0..=20u16 {
            let pos = (10 + i32::from(c), 10);
            let cell = projection
                .cells
                .get(&pos)
                .unwrap_or_else(|| panic!("the cut wall lost its block at {pos:?}"));
            assert_eq!((cell.wall, cell.col), (1, c));
            assert_eq!((cell.nx, cell.nz), (0, -1), "the north wall faces north");
        }
        assert!(!projection.cells.contains_key(&(31, 10)));

        // The southern wall was cut at its start instead, so what survives is
        // its columns 10 to 29, and the block at node 4 is one metre past the
        // texture and left to the wall that starts there.
        for c in 10..30u16 {
            let pos = (40 - i32::from(c), 25);
            let cell = projection
                .cells
                .get(&pos)
                .unwrap_or_else(|| panic!("the far end of the cut wall lost {pos:?}"));
            assert_eq!((cell.wall, cell.col), (2, c));
            assert_eq!((cell.nx, cell.nz), (0, 1), "the south wall faces south");
        }
        assert_eq!(
            projection.cells[&(10, 25)].wall,
            0,
            "the corner block stays the western wall's column 0"
        );

        assert_eq!(projection.way_cells[&55].len(), 15 + 21 + 20);
        // Every wall runs the way the lab's columns do, which the photo panels
        // crop by; the cut ones over the blocks of them the world holds.
        assert_eq!(projection.wall_dir, vec![(0, -15), (20, 0), (-20, 0)]);
    }

    #[test]
    fn way_walls_are_keyed_by_the_way_id() {
        // A 20 x 10 building way; one exported wall on its north edge, given
        // backwards so the columns run against the ring.
        let way = ProcessedWay {
            id: 55,
            tags: HashMap::from([("building".to_string(), "yes".to_string())]),
            nodes: vec![
                node(1, 10, 10),
                node(2, 30, 10),
                node(3, 30, 20),
                node(4, 10, 20),
                node(1, 10, 10),
            ],
        };
        let elements = vec![ProcessedElement::Way(way)];
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut walls = vec![FacadeWall::for_test(55, 2, 1, 20, 6, grey_tex())];

        let projection = project_cells(&mut walls, &elements, &xzbbox, 1.0);
        assert_eq!(walls[0].way_id, 55);
        assert!(projection.relation_rings.is_empty());
        // Column 0 sits at node A (east), the normal points north. The block
        // at node B is the far corner, one metre past the last column: it is
        // the next wall's, so 20 of the edge's 21 blocks are textured.
        assert_eq!(projection.way_cells[&55].len(), 20);
        assert_eq!(projection.wall_dir, vec![(-20, 0)]);
        let east = projection.cells[&(30, 10)];
        assert_eq!((east.wall, east.col, east.nx, east.nz), (0, 0, 0, -1));
        let west = projection.cells[&(11, 10)];
        assert_eq!(west.col, 19);
        assert!(!projection.cells.contains_key(&(10, 10)));
    }

    #[test]
    fn relation_walls_are_keyed_by_the_ring_the_generator_builds() {
        // A 20 x 10 building mapped as a multipolygon whose outer ring is
        // split across two member ways sharing their end nodes.
        let relation = ProcessedRelation {
            id: 7,
            tags: HashMap::from([
                ("building".to_string(), "yes".to_string()),
                ("type".to_string(), "multipolygon".to_string()),
            ]),
            members: vec![
                member(
                    100,
                    &[],
                    vec![node(1, 10, 10), node(2, 30, 10), node(3, 30, 20)],
                ),
                member(
                    101,
                    &[],
                    vec![node(3, 30, 20), node(4, 10, 20), node(1, 10, 10)],
                ),
            ],
        };
        let elements = vec![ProcessedElement::Relation(relation)];
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        // One wall on each member way, both textured from node A to node B.
        let mut walls = vec![
            FacadeWall::for_test_relation(7, 1, 2, 20, 6, grey_tex()),
            FacadeWall::for_test_relation(7, 3, 4, 20, 6, grey_tex()),
        ];

        let projection = project_cells(&mut walls, &elements, &xzbbox, 1.0);
        let ring_id = relation_ring_id(7, 0);
        assert_eq!(projection.relation_rings[&7], vec![ring_id]);
        assert!(walls.iter().all(|w| w.way_id == ring_id));
        // Every column of both edges is registered under the ring id; the
        // block at each wall's node B is the far corner past its last column.
        let columns = &projection.way_cells[&ring_id];
        for x in 10..30 {
            assert!(columns.contains(&(x, 10)), "north wall column {x}");
            assert!(columns.contains(&(x + 1, 20)), "south wall column {x}");
        }
        assert!(!columns.contains(&(30, 10)));
        assert!(!columns.contains(&(10, 20)));
        assert!(!projection.way_cells.contains_key(&7));
        // The north wall's columns run east and its normal points north; the
        // south wall's run west and its normal points south.
        let north = projection.cells[&(15, 10)];
        assert_eq!((north.wall, north.col, north.nx, north.nz), (0, 5, 0, -1));
        let south = projection.cells[&(25, 20)];
        assert_eq!((south.wall, south.col, south.nx, south.nz), (1, 5, 0, 1));
        assert_eq!(projection.wall_dir, vec![(20, 0), (-20, 0)]);

        // The store answers under the ring id, the way the wall builder asks,
        // and not under the relation id. At two blocks per metre the grid
        // answers, below that the bands.
        let colours = FnvHashMap::from_iter([(Owner::Relation(7), (200, 100, 50))]);
        let store = FacadeStore::build(walls, colours, projection, false, 2.0);
        assert!(store.has_building(ring_id));
        assert!(!store.has_building(7));
        assert_eq!(store.building_colour(ring_id), Some((200, 100, 50)));
        assert_eq!(store.building_colour(7), None);
        assert!(store.block_at(15, 3, 10, 0, ring_id, GLASS).is_some());
        assert!(store.block_at(25, 3, 20, 0, ring_id, GLASS).is_some());
        assert!(store.block_at(15, 3, 10, 0, 7, GLASS).is_none());
        assert!(store.band_block_at(15, 3, 10, 0, ring_id).is_none());
        let store = FacadeStore {
            scale: 1.0,
            ..store
        };
        assert!(store.block_at(15, 3, 10, 0, ring_id, GLASS).is_none());
        assert!(store.band_block_at(15, 3, 10, 0, ring_id).is_some());
        assert!(store.band_block_at(15, 3, 10, 0, 7).is_none());
    }

    #[test]
    fn relation_walls_off_every_ring_stay_unplaced() {
        // The relation's outer way is a closed building:part ring, which the
        // generator leaves to the standalone way, so the relation builds no
        // ring of its own.
        let part = member(
            100,
            &[("building:part", "yes")],
            vec![
                node(1, 10, 10),
                node(2, 30, 10),
                node(3, 30, 20),
                node(4, 10, 20),
                node(1, 10, 10),
            ],
        );
        let relation = ProcessedRelation {
            id: 8,
            tags: HashMap::from([
                ("building".to_string(), "yes".to_string()),
                ("type".to_string(), "multipolygon".to_string()),
            ]),
            members: vec![part],
        };
        let elements = vec![ProcessedElement::Relation(relation)];
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut walls = vec![FacadeWall::for_test_relation(8, 1, 2, 20, 6, grey_tex())];

        let projection = project_cells(&mut walls, &elements, &xzbbox, 1.0);
        assert_eq!(walls[0].way_id, UNPLACED);
        assert!(projection.cells.is_empty());
        let store = FacadeStore::build(walls, FnvHashMap::default(), projection, false, 1.0);
        assert!(!store.has_building(UNPLACED));
        assert!(!store.has_building(8));
    }

    #[test]
    fn a_wall_merged_over_five_ring_edges_and_cut_into_pieces_tiles_the_ring_in_order() {
        // r147094's south wall: three exported pieces, each listing all five
        // OSM edges with the metres they span in that piece's frame.
        let elements = vec![ProcessedElement::Relation(test_fixtures::r147094_relation())];
        let xzbbox = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let mut walls: Vec<FacadeWall> = test_fixtures::WALL8_PIECES
            .iter()
            .map(|&(start_m, col0_m, cols)| {
                let edges: Vec<(u64, u64, f64, f64)> = test_fixtures::WALL8_EDGES
                    .iter()
                    .map(|&(a, b, s0, s1)| (a, b, s0 - start_m, s1 - start_m))
                    .collect();
                FacadeWall::for_test_relation_edges(147094, &edges, col0_m, cols, 18, None)
            })
            .collect();

        let projection = project_cells(&mut walls, &elements, &xzbbox, 1.0);
        let ring_id = relation_ring_id(147094, 0);
        assert_eq!(projection.relation_rings[&147094], vec![ring_id]);
        assert!(walls.iter().all(|w| w.way_id == ring_id));
        // Every piece lists all five edges, so each runs from the first
        // edge's node A to the last edge's node B: 65 m east.
        assert_eq!(projection.wall_dir, vec![(65, 0); 3]);

        // Walking the ring from node 21486944 at x = 20, the pieces follow
        // each other and each hands out its columns 0..cols exactly once, in
        // order. The first 2.4 m lie before piece 0's texture and the last
        // metre past piece 2's, so those blocks keep the base wall.
        let expected = |i: i32| -> Option<(u32, u16)> {
            match i {
                3..=19 => Some((0, (i - 3) as u16)),
                20..=42 => Some((1, (i - 20) as u16)),
                43..=63 => Some((2, (i - 43) as u16)),
                _ => None,
            }
        };
        for i in 0..=65 {
            let got = projection.cells.get(&(20 + i, 60)).map(|c| (c.wall, c.col));
            assert_eq!(got, expected(i), "block {i} m east of node 21486944");
        }
        // No block off this wall is textured.
        let mut strays: Vec<_> = projection
            .cells
            .iter()
            .filter(|((x, z), _)| *z != 60 || !(20..=85).contains(x))
            .map(|(pos, c)| (*pos, c.wall, c.col))
            .collect();
        strays.sort();
        assert!(strays.is_empty(), "textured off the wall: {strays:?}");
        assert_eq!(projection.cells.len(), 17 + 23 + 21);
        // The building's column list holds the same blocks (a node two edges
        // share is listed once per edge).
        let mut columns = projection.way_cells[&ring_id].clone();
        columns.sort();
        columns.dedup();
        assert_eq!(columns.len(), 17 + 23 + 21);
        // The building lies north of this wall, so every normal points south.
        assert!(projection.cells.values().all(|c| (c.nx, c.nz) == (0, 1)));
    }

    #[test]
    fn row_bands_take_the_median_wall_colour_and_borrow_from_the_nearest_row() {
        let wall = |rgb: RGBTuple| (rgb, Class::Wall);
        let window = ((0, 0, 0), Class::Window);
        let none = ((0, 0, 0), Class::NoData);
        // Four rows of three: wall cells only in rows 0 and 3.
        let cells = vec![
            wall((10, 200, 30)),
            wall((20, 100, 40)),
            wall((30, 150, 50)),
            window,
            window,
            window,
            none,
            none,
            none,
            wall((90, 90, 90)),
            none,
            window,
        ];
        assert_eq!(
            row_bands(&cells, 3, 4),
            vec![(20, 150, 40), (20, 150, 40), (90, 90, 90), (90, 90, 90)]
        );
        // Two rows of wall cells the same distance away: the lower one wins.
        let cells = vec![wall((1, 1, 1)), none, wall((2, 2, 2))];
        assert_eq!(
            row_bands(&cells, 1, 3),
            vec![(1, 1, 1), (2, 2, 2), (2, 2, 2)]
        );
        // A window in a row of walls does not enter the median.
        let cells = vec![wall((10, 10, 10)), window, wall((50, 50, 50))];
        assert_eq!(row_bands(&cells, 3, 1), vec![(50, 50, 50)]);
        // No wall cell anywhere: no bands.
        assert!(row_bands(&[window, none], 2, 1).is_empty());
    }

    #[test]
    fn below_two_blocks_per_metre_only_the_band_colours_apply() {
        // A 20 x 10 building way with one exported wall on its north edge:
        // six rows, the upper three light, the lower three dark, and one
        // window cell in the bottom row at column 5.
        let light: RGBTuple = (230, 230, 230);
        let dark: RGBTuple = (60, 60, 60);
        let build = |scale: f64| {
            let way = ProcessedWay {
                id: 56,
                tags: HashMap::from([("building".to_string(), "yes".to_string())]),
                nodes: vec![
                    node(1, 10, 10),
                    node(2, 30, 10),
                    node(3, 30, 20),
                    node(4, 10, 20),
                    node(1, 10, 10),
                ],
            };
            let elements = vec![ProcessedElement::Way(way)];
            let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
            let cells: Vec<(RGBTuple, Class)> = (0..6 * 20)
                .map(|i| match (i / 20, i % 20) {
                    (5, 5) => ((0, 0, 0), Class::Window),
                    (row, _) if row < 3 => (light, Class::Wall),
                    _ => (dark, Class::Wall),
                })
                .collect();
            let mut walls = vec![FacadeWall::for_test_cells(56, 1, 2, 20, 6, cells)];
            let projection = project_cells(&mut walls, &elements, &xzbbox, 1.0);
            let colours = FnvHashMap::from_iter([(Owner::Way(56), (200, 100, 50))]);
            FacadeStore::build(walls, colours, projection, false, scale)
        };

        let store = build(1.0);
        assert!(store.colour_only());
        assert!(store.has_building(56));
        assert_eq!(store.building_colour(56), Some((200, 100, 50)));
        // The grid is off: no block for any cell, the window included.
        assert_eq!(store.block_at(15, 1, 10, 0, 56, GLASS), None);
        assert_eq!(store.block_at(15, 4, 10, 0, 56, GLASS), None);
        // The bands are on: the first three wall rows dark, the next three
        // light, the window cell like the rest of its row.
        let (light_block, dark_block) =
            (facade_block_for_color(light), facade_block_for_color(dark));
        assert_eq!(store.band_block_at(15, 1, 10, 0, 56), Some(dark_block));
        assert_eq!(store.band_block_at(15, 3, 10, 0, 56), Some(dark_block));
        assert_eq!(store.band_block_at(15, 4, 10, 0, 56), Some(light_block));
        assert_eq!(store.band_block_at(15, 6, 10, 0, 56), Some(light_block));
        // Above the textured rows, below the first wall block, off the wall
        // and for another building: nothing.
        assert_eq!(store.band_block_at(15, 7, 10, 0, 56), None);
        assert_eq!(store.band_block_at(15, 0, 10, 0, 56), None);
        assert_eq!(store.band_block_at(15, 1, 20, 0, 56), None);
        assert_eq!(store.band_block_at(15, 1, 10, 0, 57), None);

        // From two blocks per metre up it is the other way round: the grid
        // applies cell by cell and the bands do not.
        let store = build(2.0);
        assert!(!store.colour_only());
        assert_eq!(store.block_at(15, 1, 10, 0, 56, GLASS), Some(GLASS));
        assert_eq!(store.block_at(15, 2, 10, 0, 56, GLASS), Some(GLASS));
        assert_eq!(store.block_at(16, 1, 10, 0, 56, GLASS), Some(dark_block));
        assert_eq!(store.block_at(15, 7, 10, 0, 56, GLASS), Some(light_block));
        assert_eq!(store.band_block_at(15, 1, 10, 0, 56), None);
    }
}
