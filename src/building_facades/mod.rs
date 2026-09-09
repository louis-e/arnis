//! Preset facade photographs on every building, with no token and no download.
//!
//! `mapillary/displays.rs` already hangs a photograph flat on a wall as an
//! `item_display` entity, bakes it into the world's resource pack and places it
//! on the wall's true line whatever angle that line runs at. What it cannot do
//! is cover a building nobody has driven past with a camera. This module feeds
//! the same mechanism from a set of premade facade photographs picked by what
//! kind of building it is, so a world has facades everywhere or nowhere rather
//! than only along the streets Mapillary has been down.
//!
//! Nothing about the hanging is duplicated: [`displays::hang_wall`] does the
//! cutting, the entity and the pack for both sources, and the panels of both
//! end up in the one registry that writes `resources.zip`.
//!
//! # Which source wins
//!
//! Mapillary wins for a wall it has, and this fills the rest. That is a rule
//! and not an accident: [`collect`] drops every world column the facade store
//! covers for this building (`facades::photo_column`, the same test the
//! generator uses to decide which columns are built as a plain shell) before
//! it makes a candidate of anything. A corner building photographed on one
//! side keeps the photograph on that side and gets a preset facade on the
//! other, because the drop is per column and the remaining columns are split
//! into runs. A wall covered end to end produces no candidate at all.
//!
//! # The three steps
//!
//! The same shape as `displays.rs`, for the same reasons:
//!
//! * [`collect`] runs before a building's wall ring is built and records one
//!   candidate per run of wall the presets may cover, from whichever tile
//!   thread got there first. It chooses the building's texture here, so every
//!   wall of one building shows one building. It runs first because the columns
//!   it returns are the ones the generator builds as a plain shell: a column
//!   flattened for a photograph that never comes loses its window, its plinth
//!   and its accent line for nothing.
//! * A building drawn as several `building:part` elements is still one
//!   building. Each part is a separate element with its own id, its own
//!   height and its own corner, so left alone they choose a photograph each
//!   and the building comes out in two skins. They are held together by the
//!   shared style seed the generator already dresses them from
//!   (`osm_parser::PartGroups`: the parent `type=building` relation, or the
//!   `building` outline the part's centroid falls inside), which is the same
//!   thing that gives them one wall block and one window phase. The group's
//!   tallest member decides, and the rest of it hangs that member's
//!   photograph. See [`GroupPick`].
//! * [`flush_region`] and [`finalize`] settle the candidates against the
//!   finished world: a wall buried in a hillside is dropped, the picture is
//!   fitted to the wall, and the panels are hung. A region has to be settled
//!   before stream-to-disk eviction drops it, or the entities written into it
//!   are lost.
//! * The pack is written by `displays::write_packs`, which both sources share.

pub mod choose;
pub mod fit;
pub mod manifest;

use std::sync::{Arc, Mutex};

use fnv::{FnvHashMap, FnvHashSet};

use crate::bresenham::bresenham_line;
use crate::element_processing::buildings::BuildingCategory;
use crate::mapillary::displays::{self, cell_step, outward_normal, right_of};
use crate::mapillary::facades;
use crate::osm_parser::ProcessedNode;
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};
use crate::world_editor::WorldEditor;

use choose::Choice;
use fit::Fit;
use manifest::FacadeSet;

/// Shortest run of wall that gets a panel, in blocks. Below this the panel is
/// a sliver between two photographed stretches and reads as a patch.
const MIN_RUN_CELLS: usize = 2;

/// Shortest wall that gets a panel, in blocks.
const MIN_WALL_BLOCKS: i32 = 2;

/// Most output pixels per real-world metre. The panels are resampled again to
/// the atlas budget when the pack is written, so going above this only costs
/// memory while the run is on: a 32 m panel would otherwise be built at 4096
/// pixels a side on a scale-4 world.
const MAX_PX_PER_M: f64 = 32.0;

/// Fewest output pixels per metre, so a small-scale world still gets an image
/// rather than a smear of four pixels.
const MIN_PX_PER_M: f64 = 4.0;

/// Whether this process prints one line per building's choice. Read once, like
/// `ARNIS_FACADE_WALL_STATS` next door, so a run without it set pays nothing.
static DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// One run of wall waiting for the finished world.
struct Candidate {
    way_id: u64,
    /// Index of this run among the building's, which names its panels.
    run: u32,
    /// Node A to node B direction of the ring segment, in world blocks.
    dir: (i32, i32),
    /// A vector pointing out of the building, from the ring's winding.
    outward: (i32, i32),
    /// Absolute y of the first wall block.
    base_y: i32,
    /// Wall rows in blocks.
    total_h: i32,
    cells: Vec<(i32, i32)>,
    /// What this element alone would hang. Kept as the fallback for a run
    /// whose building somehow decided nothing.
    choice: Choice,
    /// The building this run belongs to. See [`Registry::groups`].
    group: u64,
}

/// The member of a building whose picture the whole of it wears.
///
/// The tallest one, ties broken by the lower element id, so the pick is a
/// function of the members and not of the order the tile threads reached
/// them. Tallest rather than largest because of how the picture is fitted: a
/// wall shorter than the photograph is a crop from its foot, which is what a
/// podium under a tower really looks like, while a wall taller than it repeats
/// the storeys, and a low podium's picture repeated up a tower is the seam
/// nobody misses.
#[derive(Clone, Copy)]
struct GroupPick {
    wall_h_m: f64,
    element_id: u64,
    choice: Choice,
    /// Elements of this building that have chosen. One for all but the S3DB
    /// buildings, which is what the summary counts.
    members: usize,
    /// Set once a panel has been hung from this pick. After that it cannot
    /// change, or the building would end up wearing two pictures after all.
    settled: bool,
}

/// What became of the candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Buildings that chose a texture.
    pub buildings: usize,
    /// Of those, the ones that are one `building:part` of several making up
    /// one building, and so hang their building's picture rather than their
    /// own. Counted at the end, when every part is in.
    pub grouped: usize,
    /// Runs of wall collected.
    pub candidates: usize,
    /// Display entities written.
    pub panels: usize,
    /// Candidates that produced at least one panel.
    pub placed: usize,
    /// Candidates that produced none.
    pub dropped: usize,
}

struct Registry {
    enabled: bool,
    /// Blocks per metre of this run.
    scale: f64,
    set: Option<Arc<FacadeSet>>,
    /// Buildings already collected. A building is processed by every tile it
    /// overlaps, and unlike a painting a display claims no air cell, so the
    /// duplicates have to be turned away by id.
    claimed: FnvHashSet<u64>,
    /// Candidates in collection order; `None` once settled.
    candidates: Vec<Option<Candidate>>,
    /// Candidate indices by the 512-block regions their cells touch.
    by_region: FnvHashMap<(i32, i32), Vec<usize>>,
    /// One picture per part group, keyed on the shared style seed the rest of
    /// the generator already groups a building's parts by.
    groups: FnvHashMap<u64, GroupPick>,
    /// The world's block extent as `(min_x, min_z, max_x, max_z)`, or `None`
    /// when nobody has said. See [`set_world_extent`].
    world: Option<(i32, i32, i32, i32)>,
    stats: Stats,
}

/// Replaced on every generation, like the facade store: the GUI generates
/// several worlds in one process and each must see its own settings.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    enabled: false,
    scale: 1.0,
    set: None,
    claimed: FnvHashSet::with_hasher(fnv::FnvBuildHasher::new()),
    candidates: Vec::new(),
    by_region: FnvHashMap::with_hasher(fnv::FnvBuildHasher::new()),
    groups: FnvHashMap::with_hasher(fnv::FnvBuildHasher::new()),
    world: None,
    stats: Stats {
        buildings: 0,
        grouped: 0,
        candidates: 0,
        panels: 0,
        placed: 0,
        dropped: 0,
    },
});

/// Tells the run how far the world reaches, so [`collect`] can leave alone the
/// walls whose panels would fall off the end of it.
///
/// A panel hangs in the cell in front of its wall, and an entity outside the
/// world is never written: on the outermost ring of blocks, a wall facing
/// outwards can carry no photograph at all. Said after [`reset`], which clears
/// it. Left unset the world is boundless, which is what the unit tests want.
pub fn set_world_extent(bbox: &crate::coordinate_system::cartesian::XZBBox) {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).world =
        Some((bbox.min_x(), bbox.min_z(), bbox.max_x(), bbox.max_z()));
}

/// Whether this run hangs preset facades. Read once per building, not once per
/// block, so a plain mutex is enough here.
pub fn enabled() -> bool {
    REGISTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set
        .is_some()
}

/// Starts a generation: forgets the previous world's candidates and loads the
/// facade set if this run wants one.
///
/// A set that will not load is a warning and not a failed run: the buildings
/// are generated exactly as they would have been without the toggle. `px` is
/// the panel resolution in pixels per block and `scale` the world's blocks per
/// metre; together they say how many pixels one metre of building gets.
pub fn reset(want: bool, dir: Option<&std::path::Path>, px: u32, scale: f64) {
    let px_per_m = px_per_m(px, scale);
    let set = if want {
        match manifest::resolve_dir(dir) {
            Some(found) => match manifest::load(&found, px_per_m) {
                Ok((set, report)) => {
                    println!(
                        "  Preset facades: {} from {}",
                        report.summary(),
                        found.display()
                    );
                    Some(Arc::new(set))
                }
                Err(e) => {
                    warn(&format!(
                        "Preset facades: {e}. Buildings keep their blocks."
                    ));
                    None
                }
            },
            None => {
                warn(&format!(
                    "Preset facades: no {} found beside the executable or in assets/{}. \
                     Buildings keep their blocks.",
                    manifest::MANIFEST_NAME,
                    manifest::DIR_NAME
                ));
                None
            }
        }
    } else {
        None
    };
    let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    *r = Registry {
        enabled: set.is_some(),
        scale,
        set,
        claimed: FnvHashSet::default(),
        candidates: Vec::new(),
        by_region: FnvHashMap::default(),
        groups: FnvHashMap::default(),
        world: None,
        stats: Stats::default(),
    };
}

/// Output pixels per real-world metre: pixels per block times blocks per
/// metre, so one metre of building gets the pixels the panel resolution
/// promises it. Clamped, because the panels are resampled again to the atlas
/// budget when the pack is written and building them larger than that only
/// costs memory while the run is on.
fn px_per_m(px: u32, scale: f64) -> f64 {
    (f64::from(px) * scale).clamp(MIN_PX_PER_M, MAX_PX_PER_M)
}

fn warn(msg: &str) {
    eprintln!("Warning: {msg}");
    // -1 leaves the progress bar alone and only shows the text.
    emit_gui_progress_update(MESSAGE_ONLY, msg);
}

/// Twice the signed area of the ring, whose sign is its winding. Positive when
/// the outward normal of an edge running `(dx, dz)` is `(dz, -dx)`.
fn signed_area2(nodes: &[ProcessedNode]) -> i64 {
    let mut sum: i64 = 0;
    for i in 0..nodes.len() {
        let a = &nodes[i];
        let b = &nodes[(i + 1) % nodes.len()];
        sum += i64::from(a.x) * i64::from(b.z) - i64::from(b.x) * i64::from(a.z);
    }
    sum
}

/// A vector pointing out of the building for an edge running `(dx, dz)`, given
/// the ring's winding. Any outward vector does: `outward_normal` uses it for
/// the sign of a dot product and computes the true perpendicular itself.
///
/// The winding, not the polygon's centre: a centre is only outward-facing for
/// a convex building, and an L-shaped block would get its inner corner turned
/// the wrong way.
fn outward_of(dir: (i32, i32), area2: i64) -> (i32, i32) {
    if area2 >= 0 {
        (dir.1, -dir.0)
    } else {
        (-dir.1, dir.0)
    }
}

/// Splits a segment's cells into runs the presets may cover: consecutive cells
/// `photographed` says no to. Runs shorter than [`MIN_RUN_CELLS`] are dropped,
/// because a sliver between two photographed stretches reads as a patch.
///
/// This is where Mapillary wins for a wall it has. The test is per column, not
/// per wall, so a corner building photographed down one side keeps the
/// photograph there and gets a preset facade on the other side; a wall covered
/// end to end produces no run at all.
fn open_runs(
    cells: &[(i32, i32)],
    photographed: &dyn Fn(i32, i32) -> bool,
) -> Vec<Vec<(i32, i32)>> {
    let mut runs = Vec::new();
    let mut current: Vec<(i32, i32)> = Vec::new();
    for &(bx, bz) in cells {
        if photographed(bx, bz) {
            if current.len() >= MIN_RUN_CELLS {
                runs.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
            continue;
        }
        current.push((bx, bz));
    }
    if current.len() >= MIN_RUN_CELLS {
        runs.push(current);
    }
    runs
}

/// One run of wall a preset panel may cover: the direction of the ring segment
/// it came from, the way out of the building, and its cells.
struct Run {
    dir: (i32, i32),
    outward: (i32, i32),
    cells: Vec<(i32, i32)>,
}

/// Whether a run can carry a panel at all given the world's extent.
///
/// The quad hangs in the cell in front of the wall, and `add_item_display`
/// writes nothing outside the world, so a run whose whole front is off the edge
/// of the generated area gets no panel however well it fits. That edge is the
/// outermost ring of blocks of every world, and a bbox cuts buildings in half
/// along it: on the Munich test box, all 24 of the runs that were collected and
/// then produced nothing were these, and each one cost its wall its windows.
fn front_reaches_the_world(
    world: Option<(i32, i32, i32, i32)>,
    outward: (i32, i32),
    cells: &[(i32, i32)],
) -> bool {
    let Some((min_x, min_z, max_x, max_z)) = world else {
        return true;
    };
    let (ox, oz) = (outward.0.signum(), outward.1.signum());
    cells.iter().any(|&(bx, bz)| {
        let (fx, fz) = (bx + ox, bz + oz);
        fx >= min_x && fx <= max_x && fz >= min_z && fz <= max_z
    })
}

/// The runs of `nodes`' ring a preset panel may cover.
///
/// A pure function of the ring, the Mapillary store and the world's extent, so
/// every tile that walks the same building gets the same runs whether or not it
/// is the one that records them, and they all build the same wall.
fn ring_runs(
    nodes: &[ProcessedNode],
    element_id: u64,
    world: Option<(i32, i32, i32, i32)>,
) -> Vec<Run> {
    let photographed = |bx: i32, bz: i32| facades::photo_column(bx, bz, element_id);
    let area2 = signed_area2(nodes);
    let mut out: Vec<Run> = Vec::new();
    for pair in nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let dir = (b.x - a.x, b.z - a.z);
        if dir == (0, 0) {
            continue;
        }
        let outward = outward_of(dir, area2);
        let cells: Vec<(i32, i32)> = bresenham_line(a.x, 0, a.z, b.x, 0, b.z)
            .into_iter()
            .map(|(bx, _, bz)| (bx, bz))
            .collect();
        for cells in open_runs(&cells, &photographed) {
            if !front_reaches_the_world(world, outward, &cells) {
                continue;
            }
            out.push(Run {
                dir,
                outward,
                cells,
            });
        }
    }
    out
}

/// The building `element_id` is part of.
///
/// `group_seed` is the shared style seed the rest of the generator already
/// dresses a building's parts from: the parent `type=building` relation where
/// OSM gives one, and otherwise the `building` outline whose ring the part's
/// centroid falls inside (`osm_parser::PartGroups`). An element that is a
/// building in its own right is handed its own id as the seed and so is a
/// building of one, which is why nothing has to be special-cased here.
///
/// It is the outline's own id a spatially detected group is keyed on, so an
/// outline too sparsely covered by its parts to be suppressed lands in the
/// same group as the parts standing on it rather than beside them in another.
fn group_of(group_seed: u64) -> u64 {
    // Without the hint bits, because two parts of one building can carry
    // different packed style hints and must still find each other. Same key
    // the sibling index in `data_processing` is built on.
    crate::osm_parser::seed_without_hint(group_seed)
}

/// Records the runs of `nodes`' wall ring that a preset facade may cover, and
/// returns the wall columns those runs cover.
///
/// Runs before the wall ring is built, because the returned columns are what
/// the generator builds as a plain shell: a column flattened for a photograph
/// that never comes loses its window, its plinth and its accent line for
/// nothing, and the building ends up a featureless box of its wall material.
/// So the answer is per column and not per building, and it is empty for every
/// wall the presets hang nothing on: a hole ring inside a courtyard, a ring too
/// short or too low to carry a panel, or a wall the Mapillary store already
/// owns. Nothing is written into the world here; the quads are placed once the
/// world is finished, against the terrain as it ends up rather than as the tile
/// that built the wall saw it.
///
/// `group_seed` is the building's shared style seed (`BuildingConfig`'s
/// `style_seed`), which is what makes every `building:part` of one building
/// hang one photograph rather than one each.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    editor: &mut WorldEditor,
    nodes: &[ProcessedNode],
    element_id: u64,
    group_seed: u64,
    category: BuildingCategory,
    start_y_offset: i32,
    abs_terrain_offset: i32,
    building_height: i32,
) -> Arc<FnvHashSet<(i32, i32)>> {
    if nodes.len() < 3 || building_height < MIN_WALL_BLOCKS {
        return Arc::default();
    }
    // Claimed under the lock, and then the lock is dropped: choosing the
    // picture and walking the ring are the work, and holding a process-wide
    // mutex across them would serialise every tile thread against every other.
    // A building is walked by every tile it overlaps and the first one claims
    // it, so the recording is done once whichever thread gets there first. The
    // columns are still worked out for the others, because all of them build
    // the walls and a shell that depended on who got here first would leave one
    // tile's half of a wall flat and the other's glazed.
    let (set, scale, world, first) = {
        let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled || !editor.map_decals_enabled() {
            return Arc::default();
        }
        let Some(set) = r.set.clone() else {
            return Arc::default();
        };
        let first = r.claimed.insert(element_id);
        (set, r.scale, r.world, first)
    };
    // An empty set would leave `choose` with nothing to pick, and a shell built
    // for a picture that is never chosen is a building stripped for nothing.
    if set.entries().is_empty() {
        return Arc::default();
    }

    let runs = ring_runs(nodes, element_id, world);
    let shell: Arc<FnvHashSet<(i32, i32)>> =
        Arc::new(runs.iter().flat_map(|r| r.cells.iter().copied()).collect());
    if !first || runs.is_empty() {
        return shell;
    }

    let wall_h_m = f64::from(building_height) / scale;
    // The anchor is the building's own lowest corner, so it is the same
    // whichever tile got here first and the same in a second generation of the
    // same area. The chooser turns it into the slot that keeps this building's
    // picture off its neighbours'.
    let anchor = nodes
        .iter()
        .fold((i32::MAX, i32::MAX), |(x, z), n| (x.min(n.x), z.min(n.z)));
    // No shell either: a wall stripped for a picture nobody picked is a
    // building left plainer than it would ever have been.
    let Some(choice) = choose::choose(&set, category, wall_h_m, element_id, anchor) else {
        return Arc::default();
    };
    let group = group_of(group_seed);
    // One line per element under `ARNIS_FACADE_DUMP=1`, which is how the
    // question "does one building wear one picture" is answered over a real
    // area: the entry printed is what this element would hang on its own, and
    // the group key says which of them are the same building.
    if *DUMP.get_or_init(|| std::env::var_os("ARNIS_FACADE_DUMP").is_some()) {
        eprintln!(
            "FACADEPICK {element_id} {group} {} {} {wall_h_m:.3} {}",
            anchor.0, anchor.1, choice.entry
        );
    }

    let base_y = start_y_offset + 1 + abs_terrain_offset;
    let mut pending: Vec<Candidate> = Vec::new();
    for run in runs {
        pending.push(Candidate {
            way_id: element_id,
            run: pending.len() as u32,
            dir: run.dir,
            outward: run.outward,
            base_y,
            total_h: building_height,
            cells: run.cells,
            choice,
            group,
        });
    }

    let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    for cand in pending {
        // The quad hangs just outside the wall, so the cell in front of each
        // wall cell counts towards the regions this candidate has to beat
        // before they are evicted.
        let (ox, oz) = (cand.outward.0.signum(), cand.outward.1.signum());
        let mut regions: Vec<(i32, i32)> = Vec::with_capacity(2);
        for &(bx, bz) in &cand.cells {
            for key in [(bx >> 9, bz >> 9), ((bx + ox) >> 9, (bz + oz) >> 9)] {
                if !regions.contains(&key) {
                    regions.push(key);
                }
            }
        }
        let index = r.candidates.len();
        for key in regions {
            r.by_region.entry(key).or_default().push(index);
        }
        r.candidates.push(Some(cand));
        r.stats.candidates += 1;
    }
    r.stats.buildings += 1;
    // The tallest member of the building decides for all of it. A pick that
    // has already hung a panel is left alone: by then part of the building is
    // wearing it, and a second picture on the rest is the very thing this is
    // here to prevent. See `flush_region` for why a taller member cannot in
    // practice still arrive by then.
    match r.groups.get_mut(&group) {
        Some(cur) => {
            cur.members += 1;
            if !cur.settled
                && wall_h_m
                    .total_cmp(&cur.wall_h_m)
                    .then_with(|| cur.element_id.cmp(&element_id))
                    .is_gt()
            {
                cur.wall_h_m = wall_h_m;
                cur.element_id = element_id;
                cur.choice = choice;
            }
        }
        None => {
            r.groups.insert(
                group,
                GroupPick {
                    wall_h_m,
                    element_id,
                    choice,
                    members: 1,
                    settled: false,
                },
            );
        }
    }
    shell
}

/// Collected elements that are one `building:part` of several making up one
/// building. The rest are buildings of one and hang their own picture.
fn grouped_parts(groups: &FnvHashMap<u64, GroupPick>) -> usize {
    groups
        .values()
        .filter(|p| p.members > 1)
        .map(|p| p.members)
        .sum()
}

/// Places one collected run: fits the picture to the wall and hands the pieces
/// to the shared display mechanism. Returns how many panels were hung.
fn place_candidate(
    editor: &mut WorldEditor,
    set: &FacadeSet,
    cand: Candidate,
    choice: Choice,
    scale: f64,
) -> usize {
    let Some(n) = outward_normal(cand.dir, cand.outward.0, cand.outward.1) else {
        return 0;
    };
    let top = cand.base_y + cand.total_h;
    if !displays::wall_is_visible(editor, cand.cells.iter().copied(), top) {
        return 0;
    }
    let Some(src) = set.image(choice.entry) else {
        return 0;
    };
    let entry = &set.entries()[choice.entry];

    // The outside viewer's left first, so a piece's crop and its world
    // position run the same way.
    let (rx, rz) = right_of(n);
    let mut cells = cand.cells;
    cells.sort_by(|a, b| {
        let along = |c: &(i32, i32)| f64::from(c.0) * rx + f64::from(c.1) * rz;
        along(a).total_cmp(&along(b))
    });

    let step = cell_step(cand.dir);
    let px_per_m = set.px_per_m();
    // Exactly the quad's own width, not an estimate of it: `quad_for` spans
    // the gaps between the cell centres plus half a step at each end, which
    // for the one-cell-per-major-axis-step run a Bresenham walk produces is
    // the cell count times the step. The picture therefore covers the panel
    // edge to edge and nothing has to be scaled to make it reach.
    let wall_w_m = cells.len() as f64 * step / scale;
    let wall_h_m = f64::from(cand.total_h) / scale;
    let fit = Fit::new(entry, px_per_m, wall_w_m, wall_h_m, choice.phase_m);

    let mut crop = |p0: i32, p1: i32, b0: i32, b1: i32| {
        // Metres along the wall from its left end, and metres up from its
        // foot. The pieces of one wall therefore read one continuous picture:
        // they address the same metres the whole wall would.
        let x0 = f64::from(p0) * step / scale;
        let x1 = f64::from(p1) * step / scale;
        let y0 = f64::from(b0) / scale;
        let y1 = f64::from(b1) / scale;
        fit.region(&src, x0, x1, y0, y1)
    };
    displays::hang_wall(
        editor,
        'b',
        cand.way_id,
        cand.run,
        &cells,
        n,
        step,
        cand.base_y,
        cand.total_h,
        &mut crop,
    )
}

/// The photograph a run hangs: its building's when it is one part of several,
/// and its own when it is a building in its own right.
fn picture_of(groups: &FnvHashMap<u64, GroupPick>, cand: &Candidate) -> Choice {
    groups
        .get(&cand.group)
        .map_or(cand.choice, |pick| pick.choice)
}

/// Settles the candidates at `indices` that are still pending.
fn place_pending(editor: &mut WorldEditor, indices: impl IntoIterator<Item = usize>) {
    let (set, scale) = {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled {
            return;
        }
        (r.set.clone(), r.scale)
    };
    let Some(set) = set else {
        return;
    };
    for i in indices {
        // The lock is released across the placement: fitting the picture and
        // writing the entity are the slow parts, and `displays::hang_wall`
        // takes its own registry lock per panel.
        let Some((cand, choice)) = ({
            let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            match r.candidates.get_mut(i).and_then(Option::take) {
                Some(cand) => {
                    let choice = picture_of(&r.groups, &cand);
                    // Hanging it settles the building: whatever else of it is
                    // still to come now hangs this same picture.
                    if let Some(pick) = r.groups.get_mut(&cand.group) {
                        pick.settled = true;
                    }
                    Some((cand, choice))
                }
                None => None,
            }
        }) else {
            continue;
        };
        let hung = place_candidate(editor, &set, cand, choice, scale);
        let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        r.stats.panels += hung;
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
///
/// This is also where a part group's picture stops being provisional, so it
/// matters when it runs: `data_processing` calls it only once every tile whose
/// own region is within one region of `(rx, rz)` has merged, and a tile
/// collects the buildings it builds before it merges. Every part of a building
/// with a wall in this region is therefore already in, since the parts of one
/// building are tens of blocks apart and a region is 512, and the group the
/// panel is hung from is the whole group.
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

/// The picture every pending candidate would hang, as (element id, entry).
///
/// What `place_pending` resolves for each of them, without placing anything:
/// the building's pick where its group has one, and the element's own choice
/// otherwise.
#[cfg(test)]
fn collected_pictures() -> Vec<(u64, usize)> {
    let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    r.candidates
        .iter()
        .flatten()
        .map(|c| (c.way_id, picture_of(&r.groups, c).entry))
        .collect()
}

/// Settles every candidate still pending against the finished world, right
/// before it is saved. `None` when the run collected nothing.
pub fn finalize(editor: &mut WorldEditor) -> Option<Report> {
    let count = {
        let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if !r.enabled || r.stats.candidates == 0 {
            return None;
        }
        r.candidates.len()
    };
    place_pending(editor, 0..count);
    // Every panel is placed, so the photographs have no reader left. Letting
    // go of them here and not at the next `reset` returns the decoded set
    // before the world save, which is the run's own high-water mark.
    if let Some(set) = REGISTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set
        .as_ref()
    {
        set.release_images();
    }
    let report = Report {
        stats: {
            let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            // Counted here and not as the parts arrive: a building is only
            // known to be made of several once the last of them is in.
            r.stats.grouped = grouped_parts(&r.groups);
            // One line per building under `ARNIS_FACADE_DUMP=1`, sorted, so
            // two generations of one area can be diffed against each other.
            if *DUMP.get_or_init(|| std::env::var_os("ARNIS_FACADE_DUMP").is_some()) {
                let mut picks: Vec<(u64, &GroupPick)> =
                    r.groups.iter().map(|(&k, p)| (k, p)).collect();
                picks.sort_by_key(|&(k, _)| k);
                for (key, p) in picks {
                    eprintln!(
                        "FACADEGROUP {key} {} {} {}",
                        p.element_id, p.choice.entry, p.members
                    );
                }
            }
            r.stats
        },
    };
    emit_gui_progress_update(MESSAGE_ONLY, &report.summary());
    Some(report)
}

/// Summary of what `finalize` wrote.
pub struct Report {
    pub stats: Stats,
}

impl Report {
    fn summary(&self) -> String {
        let s = self.stats;
        // Only said where there are any, so an area with no S3DB parts reads
        // the way it always has.
        let parts = if s.grouped == 0 {
            String::new()
        } else {
            format!(
                ", {} of them parts hanging one building's picture",
                s.grouped
            )
        };
        format!(
            "Preset facades: {} panels on {} of {} wall runs over {} buildings{parts} ({} without a usable crop)",
            s.panels, s.placed, s.candidates, s.buildings, s.dropped
        )
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "  {}", self.summary())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id: 0,
            tags: std::collections::HashMap::new(),
            x,
            z,
        }
    }

    /// A square walked anticlockwise in Arnis' frame (x east, z south).
    fn square() -> Vec<ProcessedNode> {
        vec![
            node(0, 0),
            node(10, 0),
            node(10, 10),
            node(0, 10),
            node(0, 0),
        ]
    }

    #[test]
    fn the_winding_turns_every_edge_outward() {
        let ring = square();
        let area2 = signed_area2(&ring);
        // The interior is 0 < x < 10, 0 < z < 10, so each edge's outward
        // vector has to point away from it.
        let checks = [
            ((0, 0), (10, 0), (0, -1)),  // north edge, outward is -z
            ((10, 0), (10, 10), (1, 0)), // east edge, outward is +x
            ((10, 10), (0, 10), (0, 1)), // south edge, outward is +z
            ((0, 10), (0, 0), (-1, 0)),  // west edge, outward is -x
        ];
        for ((ax, az), (bx, bz), want) in checks {
            let dir = (bx - ax, bz - az);
            let out = outward_of(dir, area2);
            assert_eq!((out.0.signum(), out.1.signum()), want, "{dir:?}");
            // And the true perpendicular the display mechanism derives from it
            // points the same way.
            let n = outward_normal(dir, out.0, out.1).unwrap();
            assert!(n.0 * f64::from(want.0) + n.1 * f64::from(want.1) > 0.9);
        }
    }

    #[test]
    fn the_winding_is_read_from_the_ring_and_not_assumed() {
        let mut reversed = square();
        reversed.reverse();
        let area2 = signed_area2(&reversed);
        assert!(
            area2 * signed_area2(&square()) < 0,
            "the two windings differ"
        );
        // Walked the other way the same edge runs the other way too, so the
        // outward vector is still outward.
        let dir = (-10, 0); // the north edge, now running west
        let out = outward_of(dir, area2);
        assert_eq!((out.0.signum(), out.1.signum()), (0, -1));
    }

    #[test]
    fn a_degenerate_ring_has_no_winding_and_no_panels() {
        let flat = vec![node(0, 0), node(10, 0), node(0, 0)];
        assert_eq!(signed_area2(&flat), 0);
        // Zero area still gives a definite answer rather than a panic; the
        // wall is a line and the side it faces does not matter.
        let out = outward_of((10, 0), 0);
        assert_eq!(out, (0, -10));
    }

    #[test]
    fn a_wall_with_no_photograph_is_one_run() {
        let cells: Vec<(i32, i32)> = (0..10).map(|i| (i, 0)).collect();
        let runs = open_runs(&cells, &|_, _| false);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 10);
    }

    #[test]
    fn mapillary_wins_the_columns_it_has_and_the_presets_fill_the_rest() {
        let cells: Vec<(i32, i32)> = (0..20).map(|i| (i, 0)).collect();
        // A photograph covering the middle of the wall, as a corner building
        // shot from one street gets.
        let runs = open_runs(&cells, &|bx, _| (6..14).contains(&bx));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0], (0..6).map(|i| (i, 0)).collect::<Vec<_>>());
        assert_eq!(runs[1], (14..20).map(|i| (i, 0)).collect::<Vec<_>>());
        // Covered end to end, nothing is left for the presets.
        assert!(open_runs(&cells, &|_, _| true).is_empty());
    }

    #[test]
    fn a_wall_on_the_edge_of_the_world_carries_no_panel() {
        // The quad hangs in the cell in front of the wall, and nothing is
        // written outside the world, so the outermost ring of blocks can carry
        // no photograph facing outwards. `collect` has to see that before the
        // wall is built, or the wall is flattened for a panel that is then
        // never placed.
        let world = Some((0, 0, 99, 99));
        let south: Vec<(i32, i32)> = (10..20).map(|i| (i, 0)).collect();
        assert!(
            !front_reaches_the_world(world, (0, -1), &south),
            "a south-facing wall on z = 0 has its whole front outside"
        );
        assert!(
            front_reaches_the_world(world, (0, 1), &south),
            "the same wall facing inwards is fine"
        );
        assert!(
            front_reaches_the_world(
                world,
                (0, -1),
                &(10..20).map(|i| (i, 1)).collect::<Vec<_>>()
            ),
            "one block in, the front is inside the world again"
        );
        // A wall running away from the edge only needs one cell that reaches.
        let corner: Vec<(i32, i32)> = (0..4).map(|i| (i, 0)).collect();
        assert!(!front_reaches_the_world(world, (0, -1), &corner));
        assert!(
            front_reaches_the_world(None, (0, -1), &corner),
            "unset means boundless"
        );
    }

    #[test]
    fn a_run_of_one_cell_is_dropped() {
        // A sliver between two photographed stretches reads as a patch, so it
        // is left to the blocks.
        assert!(open_runs(&[(0, 0)], &|_, _| false).is_empty());
        assert_eq!(open_runs(&[(0, 0), (1, 0)], &|_, _| false).len(), 1);
        let cells: Vec<(i32, i32)> = (0..10).map(|i| (i, 0)).collect();
        assert_eq!(open_runs(&cells, &|bx, _| bx == 1).len(), 1);
    }

    #[test]
    fn the_feature_is_off_until_it_is_switched_on() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        reset(false, None, 16, 1.0);
        assert!(!enabled());
        let stats = REGISTRY.lock().unwrap().stats;
        assert_eq!(stats, Stats::default());
    }

    /// A building nobody made a part of is a building of one: the seed the
    /// generator hands it is its own id.
    #[test]
    fn a_building_that_is_nobodys_part_is_a_building_of_one() {
        assert_eq!(group_of(4712), 4712);
        // A relation ring is built under a synthetic id which is also its own
        // seed, so a multipolygon's rings stay separate buildings.
        let ring = crate::element_processing::buildings::relation_ring_id(147094, 0);
        assert_eq!(group_of(ring), ring, "the hint bits have to be free here");
        // A part carries its building's seed, and the hint bits packed into it
        // must not stop two parts of one building finding each other.
        let outline = 8811u64;
        let masonry = outline | (1 << 61);
        assert_eq!(group_of(outline), outline);
        assert_eq!(group_of(masonry), outline);
    }

    /// A facade set with several residential pictures of different heights,
    /// enough that two walls of different heights really do choose
    /// differently. The files are empty: the loader only checks that the
    /// picture the manifest names is there, and nothing here hangs a panel.
    fn set_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut textures = String::new();
        for (i, (w, h, storeys)) in [
            (10.0, 6.2, 2),
            (11.0, 9.3, 3),
            (12.0, 12.4, 4),
            (13.0, 15.5, 5),
            (14.0, 18.6, 6),
            (24.0, 27.9, 9),
            (25.0, 62.0, 20),
            (26.0, 93.0, 30),
        ]
        .iter()
        .enumerate()
        {
            let file = format!("r{i:02}.png");
            std::fs::write(dir.path().join(&file), b"").unwrap();
            if i > 0 {
                textures.push(',');
            }
            textures.push_str(&format!(
                r#"{{"file": "{file}", "categories": ["Default"], "metres_wide": {w},
                    "metres_tall": {h}, "storeys": {storeys},
                    "tiles_horizontally": true, "has_ground_floor": true}}"#
            ));
        }
        std::fs::write(
            dir.path().join(manifest::MANIFEST_NAME),
            format!(r#"{{"version": 1, "textures": [{textures}]}}"#),
        )
        .unwrap();
        dir
    }

    /// A square ring `side` blocks across with its lowest corner at `(x, z)`.
    fn part_ring(x: i32, z: i32, side: i32) -> Vec<ProcessedNode> {
        vec![
            node(x, z),
            node(x + side, z),
            node(x + side, z + side),
            node(x, z + side),
            node(x, z),
        ]
    }

    /// One `building:part` of the building whose shared seed is `group_seed`.
    fn collect_part(
        editor: &mut WorldEditor,
        id: u64,
        group_seed: u64,
        x: i32,
        z: i32,
        height: i32,
    ) {
        collect(
            editor,
            &part_ring(x, z, 20),
            id,
            group_seed,
            BuildingCategory::Default,
            0,
            0,
            height,
        );
    }

    /// The tower, the wing and the podium of one S3DB building, at three
    /// corners and three heights: left to themselves they choose three
    /// photographs, and the building comes out in three skins.
    #[test]
    fn the_parts_of_one_building_hang_one_photograph() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = set_dir();
        let xz =
            crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(0, 0, 200, 200).unwrap();

        // What the three would have chosen on their own, which is what the
        // owner is complaining about; if these ever agree the test below
        // stops proving anything.
        reset(true, Some(dir.path()), 16, 1.0);
        let set = REGISTRY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set
            .clone()
            .unwrap();
        let alone: Vec<usize> = [(901u64, 0, 0, 90), (902, 40, 0, 30), (903, 0, 40, 12)]
            .iter()
            .map(|&(id, x, z, h)| {
                choose::choose(&set, BuildingCategory::Default, f64::from(h), id, (x, z))
                    .unwrap()
                    .entry
            })
            .collect();
        assert_eq!(
            alone
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3,
            "the three parts have to disagree for this test to mean anything"
        );

        // The same three as parts of one building, offered in every order, so
        // the answer cannot come from whichever tile thread arrived first.
        let orders: [[usize; 3]; 3] = [[0, 1, 2], [2, 1, 0], [1, 2, 0]];
        let mut agreed: Vec<usize> = Vec::new();
        for order in orders {
            reset(true, Some(dir.path()), 16, 1.0);
            let mut editor = crate::element_processing::building_test_support::test_editor(&xz);
            editor.set_map_decals(true);
            for i in order {
                let (id, x, z, h) = [(901u64, 0, 0, 90), (902, 40, 0, 30), (903, 0, 40, 12)][i];
                collect_part(&mut editor, id, 8811, x, z, h);
            }
            let pictures = collected_pictures();
            let used: std::collections::BTreeSet<usize> =
                pictures.iter().map(|&(_, e)| e).collect();
            assert_eq!(used.len(), 1, "one building, {} photographs", used.len());
            assert_eq!(
                pictures
                    .iter()
                    .map(|&(id, _)| id)
                    .collect::<std::collections::BTreeSet<u64>>()
                    .len(),
                3,
                "all three parts have to be in the count"
            );
            let parts = {
                let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
                grouped_parts(&r.groups)
            };
            assert_eq!(parts, 3, "the summary did not see three parts of one");
            agreed.push(*used.iter().next().unwrap());
        }
        assert!(
            agreed.windows(2).all(|w| w[0] == w[1]),
            "the order the parts arrived in changed the picture: {agreed:?}"
        );
        // The tallest part decides: a podium wearing the bottom of a tower's
        // photograph is right, a tower repeating a podium's is not.
        assert_eq!(agreed[0], alone[0], "the 90 block part did not decide");
        reset(false, None, 16, 1.0);
    }

    /// Two buildings that merely stand next to each other are two buildings.
    /// Joining them is the worse of the two errors, so the group has to come
    /// from what OSM says and never from proximity.
    #[test]
    fn two_neighbouring_buildings_keep_their_own_photographs() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = set_dir();
        let xz =
            crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(0, 0, 200, 200).unwrap();
        reset(true, Some(dir.path()), 16, 1.0);
        let set = REGISTRY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set
            .clone()
            .unwrap();
        let mut editor = crate::element_processing::building_test_support::test_editor(&xz);
        editor.set_map_decals(true);
        // Touching walls, one block apart, one of them twice the other's
        // height, and each carrying its own seed because OSM says nothing
        // about them belonging together.
        collect_part(&mut editor, 901, 901, 0, 0, 30);
        collect_part(&mut editor, 902, 902, 21, 0, 60);
        // Each still hangs exactly what it chose for itself. Not that the two
        // differ: whether two neighbours land on one picture is the lattice's
        // business (see `choose`), and it is allowed to say yes. What must not
        // happen is the taller one deciding for the shorter.
        for (id, anchor, h) in [(901u64, (0, 0), 30.0), (902, (21, 0), 60.0)] {
            let own = choose::choose(&set, BuildingCategory::Default, h, id, anchor).unwrap();
            let hung: Vec<usize> = collected_pictures()
                .iter()
                .filter(|&&(w, _)| w == id)
                .map(|&(_, e)| e)
                .collect();
            assert!(!hung.is_empty(), "{id} hung nothing");
            assert!(
                hung.iter().all(|&e| e == own.entry),
                "{id} was made to wear its neighbour's picture: {hung:?}"
            );
        }
        let parts = {
            let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            grouped_parts(&r.groups)
        };
        assert_eq!(parts, 0, "two buildings counted as one building's parts");
        reset(false, None, 16, 1.0);
    }

    #[test]
    fn a_missing_manifest_leaves_generation_working() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let empty = tempfile::tempdir().unwrap();
        // Asked for, and not there: the feature turns itself off and says so
        // rather than failing the run.
        reset(true, Some(empty.path()), 16, 1.0);
        assert!(!enabled());
        reset(false, None, 16, 1.0);
    }

    #[test]
    fn an_unreadable_manifest_leaves_generation_working() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(manifest::MANIFEST_NAME), b"{oh dear").unwrap();
        reset(true, Some(dir.path()), 16, 1.0);
        assert!(!enabled());

        // And an empty one, which parses but describes nothing.
        std::fs::write(
            dir.path().join(manifest::MANIFEST_NAME),
            br#"{"version": 1, "textures": []}"#,
        )
        .unwrap();
        reset(true, Some(dir.path()), 16, 1.0);
        assert!(!enabled());
        reset(false, None, 16, 1.0);
    }

    #[test]
    fn the_pixels_per_metre_follow_the_world_scale_and_stay_in_range() {
        for (px, scale, want) in [
            (16u32, 1.0f64, 16.0f64),
            (8, 2.0, 16.0),
            (32, 4.0, MAX_PX_PER_M),
            (4, 0.25, MIN_PX_PER_M),
        ] {
            assert!(
                (px_per_m(px, scale) - want).abs() < 1e-9,
                "{px} px per block at {scale} blocks per metre"
            );
        }
    }
}
