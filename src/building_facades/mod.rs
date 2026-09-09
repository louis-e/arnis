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
//! A wall that faces into the building next door is split off the same way,
//! column by column, when the building pass says which columns those are
//! ([`collect_facing`]). Those columns are hung against the finished world:
//! not at all where the neighbour is as tall, since the panel would hang
//! inside it, and from its roof up where it is lower.
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
//!   are lost. `finalize` first sizes the atlas from every candidate still
//!   pending and brings the set to the resolution the pack will end at, so
//!   the crops are built at that size rather than shrunk to it afterwards.
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

/// Panel names per run: a party wall is hung in pieces that start at
/// different heights along it, one per neighbour it stands against, and each
/// piece needs a name of its own. Sixteen is far more than a wall ever meets.
const SUBRUNS: u32 = 16;

/// How many rows the neighbour's height may vary along one piece of a party
/// wall. A pitched roof or a parapet against the wall steps up and down by a
/// row or two per cell, and a piece per step would be one cell wide and too
/// short to hang, so a stretch is allowed this much spread and hung from its
/// highest point. What that costs is at most this many rows of wall above the
/// lower end of the roof, which stay the party wall's own blocks.
const ROOF_STEP: i32 = 2;

/// Whether this process prints one line per building's choice. Read once, like
/// `ARNIS_FACADE_WALL_STATS` next door, so a run without it set pays nothing.
static DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// Whether `ARNIS_FACADE_DUMP` is set. One branch on a cached bool, so the
/// lines below cost a run without it nothing.
fn dump() -> bool {
    *DUMP.get_or_init(|| std::env::var_os("ARNIS_FACADE_DUMP").is_some())
}

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
    /// Whether the run faces into another building, and so is hung only
    /// above that building's roof. See [`collect_facing`].
    party: bool,
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
    /// Of those, the runs that face a building as tall as their own wall,
    /// which is the one reason a run is meant to produce nothing.
    pub behind: usize,
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
        behind: 0,
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
    if dump() {
        eprintln!(
            "FACADEWORLD {} {} {} {}",
            bbox.min_x(),
            bbox.min_z(),
            bbox.max_x(),
            bbox.max_z()
        );
    }
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
                    warn(
                        "Facades: textures would not load",
                        &format!("Preset facades: {e}. Buildings keep their blocks."),
                    );
                    None
                }
            },
            None => {
                warn(
                    "Facades: textures not found",
                    &format!(
                        "Preset facades: no {} found beside the executable or in assets/{}. \
                         Buildings keep their blocks.",
                        manifest::MANIFEST_NAME,
                        manifest::DIR_NAME
                    ),
                );
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

/// The resolution the bundled set was built at (`build_facades.py`, `PPM`).
/// Decoding it any finer only spreads the same pixels over more memory.
const SET_PX_PER_M: f64 = 32.0;

/// Output pixels per real-world metre: pixels per block times blocks per
/// metre, so one metre of building gets exactly the pixels the pack gives a
/// block, and a crop gathered from the set at this is the texture the writer
/// stores, neither shrunk nor blown up. Capped at the set's own resolution:
/// at scale 4 the pack may want 64 px per metre, and a set decoded to that
/// holds four times the pixels of the photographs for no more detail. Above
/// the cap the writer blows the crop up per panel instead, which costs the
/// panel and not the whole set.
fn px_per_m(px: u32, scale: f64) -> f64 {
    (f64::from(px) * scale).min(SET_PX_PER_M)
}

/// One warning in the two lengths its two destinations have room for: see the
/// twin of this in `mapillary::displays`. `short` has to stand on its own and
/// `long` has to hold everything `short` leaves out.
fn warn(short: &str, long: &str) {
    eprintln!("Warning: {long}");
    // -1 leaves the progress bar alone and only shows the text.
    emit_gui_progress_update(MESSAGE_ONLY, short);
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
/// `photographed` says no to, and consecutive cells that agree about
/// `blocked`, which is the flag each run comes back with. Runs shorter than
/// [`MIN_RUN_CELLS`] are dropped, because a sliver between two photographed
/// stretches reads as a patch.
///
/// This is where Mapillary wins for a wall it has. The test is per column, not
/// per wall, so a corner building photographed down one side keeps the
/// photograph there and gets a preset facade on the other side; a wall covered
/// end to end produces no run at all. A blocked stretch is kept as a run of
/// its own rather than dropped, because what is hung on it is decided against
/// the finished world: nothing where the neighbour is as tall, and the storeys
/// above its roof where it is not.
fn open_runs(
    cells: &[(i32, i32)],
    photographed: &dyn Fn(i32, i32) -> bool,
    blocked: &dyn Fn(i32, i32) -> bool,
) -> Vec<(Vec<(i32, i32)>, bool)> {
    fn close(current: &mut Vec<(i32, i32)>, party: bool, runs: &mut Vec<(Vec<(i32, i32)>, bool)>) {
        if current.len() >= MIN_RUN_CELLS {
            runs.push((std::mem::take(current), party));
        } else {
            current.clear();
        }
    }
    let mut runs = Vec::new();
    let mut current: Vec<(i32, i32)> = Vec::new();
    let mut party = false;
    for &(bx, bz) in cells {
        if photographed(bx, bz) {
            close(&mut current, party, &mut runs);
            continue;
        }
        let now = blocked(bx, bz);
        if now != party {
            close(&mut current, party, &mut runs);
            party = now;
        }
        current.push((bx, bz));
    }
    close(&mut current, party, &mut runs);
    runs
}

/// One run of wall a preset panel may cover: the direction of the ring segment
/// it came from, the way out of the building, and its cells.
struct Run {
    dir: (i32, i32),
    outward: (i32, i32),
    cells: Vec<(i32, i32)>,
    /// Whether every cell faces into another building.
    party: bool,
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

/// Which wall columns face into another building, and so can carry no panel
/// below that building's roof. Given a column and the axis-signed step out of the
/// building from it, so the corner cell two ring segments share is answered
/// for each of them separately: it is blocked along the party wall and free
/// along the street wall, and only a per-direction answer keeps the street
/// panel from stopping one block short of the corner.
pub type Blocked<'a> = &'a dyn Fn(i32, i32, (i32, i32)) -> bool;

/// The runs of `nodes`' ring a preset panel may cover.
///
/// A pure function of the ring, the Mapillary store, `blocked` and the world's
/// extent, so every tile that walks the same building gets the same runs
/// whether or not it is the one that records them, and they all build the same
/// wall.
fn ring_runs(
    nodes: &[ProcessedNode],
    element_id: u64,
    world: Option<(i32, i32, i32, i32)>,
    blocked: Blocked<'_>,
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
        let step_out = (outward.0.signum(), outward.1.signum());
        let blocked_here = |bx: i32, bz: i32| blocked(bx, bz, step_out);
        let cells: Vec<(i32, i32)> = bresenham_line(a.x, 0, a.z, b.x, 0, b.z)
            .into_iter()
            .map(|(bx, _, bz)| (bx, bz))
            .collect();
        for (cells, party) in open_runs(&cells, &photographed, &blocked_here) {
            if !front_reaches_the_world(world, outward, &cells) {
                continue;
            }
            out.push(Run {
                dir,
                outward,
                cells,
                party,
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
///
/// `blocked` says which wall columns face into another building.
///
/// A panel hangs in the cell in front of its wall, so a wall that shares its
/// line with the house next door, or stands inside a mall, or lies under an
/// outline that overlaps this one, would put its photograph inside that other
/// building: unseen from the street and wrong from inside. The wall pass
/// already knows those columns, it is the test behind `FacadePlan::is_party`,
/// which is why they are asked for here rather than guessed from the finished
/// blocks: the neighbour's interior is hollow, so the blocks alone cannot tell
/// a wall inside it from a wall in the open. What the blocks can tell, once
/// the world is built, is how tall the neighbour is, and that is how a blocked
/// run is hung: not at all where the neighbour reaches the top, and from its
/// roof up where it does not, which is the part of a party wall a street does
/// see. On the Munich test box, two fifths of the panel rows hung before this
/// were inside the building next door.
#[allow(clippy::too_many_arguments)]
pub fn collect_facing(
    editor: &mut WorldEditor,
    nodes: &[ProcessedNode],
    element_id: u64,
    group_seed: u64,
    category: BuildingCategory,
    start_y_offset: i32,
    abs_terrain_offset: i32,
    building_height: i32,
    blocked: Blocked<'_>,
) -> Arc<FnvHashSet<(i32, i32)>> {
    // The ring as the generator sees it, once per tile that walks it, so the
    // world can be read back against every building and not only the ones
    // that got a candidate. Deduplicated by id downstream.
    if dump() {
        let ring: Vec<String> = nodes.iter().map(|n| format!("{},{}", n.x, n.z)).collect();
        eprintln!(
            "FACADEWALK {element_id} {} {} {building_height} {start_y_offset} {abs_terrain_offset} {}",
            group_of(group_seed),
            manifest::category_name(category),
            ring.join(" ")
        );
    }
    if nodes.len() < 3 || building_height < MIN_WALL_BLOCKS {
        if dump() {
            eprintln!("FACADESKIP {element_id} short");
        }
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

    let runs = ring_runs(nodes, element_id, world, blocked);
    let shell: Arc<FnvHashSet<(i32, i32)>> =
        Arc::new(runs.iter().flat_map(|r| r.cells.iter().copied()).collect());
    if !first || runs.is_empty() {
        if first && dump() {
            eprintln!("FACADESKIP {element_id} noruns");
        }
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
        if dump() {
            eprintln!("FACADESKIP {element_id} nochoice");
        }
        return Arc::default();
    };
    let group = group_of(group_seed);
    // One line per element under `ARNIS_FACADE_DUMP=1`, which is how the
    // question "does one building wear one picture" is answered over a real
    // area: the entry printed is what this element would hang on its own, and
    // the group key says which of them are the same building.
    if dump() {
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
            party: run.party,
        });
    }

    if dump() {
        for cand in &pending {
            let cells: Vec<String> = cand
                .cells
                .iter()
                .map(|c| format!("{},{}", c.0, c.1))
                .collect();
            eprintln!(
                "FACADERUN {element_id} {} {},{} {},{} {} {} {} {}",
                cand.run,
                cand.dir.0,
                cand.dir.1,
                cand.outward.0,
                cand.outward.1,
                cand.base_y,
                cand.total_h,
                if cand.party { "party" } else { "free" },
                cells.join(" ")
            );
        }
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

/// One hanging of a run: the run's cells `range` covers, hung from
/// `rows_hidden` rows up, under the panel wall number `wall`.
struct Stretch {
    range: std::ops::Range<usize>,
    rows_hidden: i32,
    wall: u32,
}

/// Where a run's panels go, read against the finished world: the wall's true
/// normal and cell step, its cells sorted along the outside viewer's right
/// with where each sits along the wall, its metres, and the stretches it is
/// hung in. What `place_candidate` hangs and what the atlas estimate sizes
/// are both read off this, so the two cannot disagree about a piece.
struct Layout {
    n: (f64, f64),
    step: f64,
    cells: Vec<(i32, i32)>,
    positions: Vec<f64>,
    wall_w_m: f64,
    wall_h_m: f64,
    stretches: Vec<Stretch>,
}

/// Works out a run's layout against the finished world, or says why it hangs
/// nothing.
fn layout(editor: &mut WorldEditor, cand: &Candidate, scale: f64) -> Result<Layout, &'static str> {
    let Some(n) = outward_normal(cand.dir, cand.outward.0, cand.outward.1) else {
        return Err("nonormal");
    };
    let top = cand.base_y + cand.total_h;
    if !displays::wall_is_visible(editor, cand.cells.iter().copied(), top) {
        return Err("buried");
    }

    // The outside viewer's left first, so a piece's crop and its world
    // position run the same way.
    let (rx, rz) = right_of(n);
    let mut cells = cand.cells.clone();
    let along = |c: &(i32, i32)| f64::from(c.0) * rx + f64::from(c.1) * rz;
    cells.sort_by(|a, b| along(a).total_cmp(&along(b)));
    let positions: Vec<f64> = cells.iter().map(along).collect();

    let step = cell_step(cand.dir);
    // The quad's own width, which is the spread of the cell centres along the
    // wall plus half a step at each end (`quad_for`), and not the cell count
    // times the step: a Bresenham walk along an oblique wall can put its last
    // cell up to one minor step short of or past where the count says, and a
    // picture built for the count is then stretched by that much to reach the
    // quad's edges. Read the same way per piece, the pieces tile the wall's
    // metres exactly and meet without a shift.
    let wall_w_m = span_m(&positions, 0, positions.len(), step, scale).1;
    let wall_h_m = f64::from(cand.total_h) / scale;

    // Rows behind the neighbour, per cell. Nothing hides a free run.
    let hidden: Vec<i32> = if cand.party {
        let (ox, oz) = (cand.outward.0.signum(), cand.outward.1.signum());
        cells
            .iter()
            .map(|&(bx, bz)| {
                rows_behind_neighbour(editor, bx, bz, ox, oz, cand.base_y, cand.total_h)
            })
            .collect()
    } else {
        vec![0; cells.len()]
    };

    // One hanging per stretch of cells hidden to about the same height, so a
    // wall against two neighbours of different heights gets a piece above
    // each, and a wall above a pitched roof gets one piece from the ridge up.
    let mut stretches: Vec<Stretch> = Vec::new();
    let mut start = 0usize;
    while start < cells.len() {
        let (mut lowest, mut highest) = (hidden[start], hidden[start]);
        let mut end = start + 1;
        while end < cells.len() && hidden[end].max(highest) - hidden[end].min(lowest) <= ROOF_STEP {
            lowest = lowest.min(hidden[end]);
            highest = highest.max(hidden[end]);
            end += 1;
        }
        let rows_hidden = highest;
        let range = start..end;
        start = end;
        if rows_hidden >= cand.total_h
            || range.len() < MIN_RUN_CELLS
            || stretches.len() >= SUBRUNS as usize
        {
            continue;
        }
        let wall = cand.run * SUBRUNS + stretches.len() as u32;
        stretches.push(Stretch {
            range,
            rows_hidden,
            wall,
        });
    }
    Ok(Layout {
        n,
        step,
        cells,
        positions,
        wall_w_m,
        wall_h_m,
        stretches,
    })
}

/// The metres of the wall a piece reads: cells `p0..p1` of `stretch` along
/// the whole run from its left end, and rows `b0..b1` of the stretch up from
/// the foot of the whole wall. The pieces of one wall, and the stretches of
/// one party wall, therefore read one continuous picture: they address the
/// same metres the whole wall would, and a stretch hung from the neighbour's
/// roof up shows the storeys that stand there rather than the ground floor
/// again.
fn piece_metres(
    lay: &Layout,
    stretch: &Stretch,
    scale: f64,
    (p0, p1): (i32, i32),
    (b0, b1): (i32, i32),
) -> (f64, f64, f64, f64) {
    let (x0, x1) = span_m(
        &lay.positions,
        stretch.range.start + p0 as usize,
        stretch.range.start + p1 as usize,
        lay.step,
        scale,
    );
    let y0 = f64::from(b0 + stretch.rows_hidden) / scale;
    let y1 = f64::from(b1 + stretch.rows_hidden) / scale;
    (x0, x1, y0, y1)
}

/// Places one collected run: fits the picture to the wall and hands the pieces
/// to the shared display mechanism. Returns how many panels were hung.
fn place_candidate(
    editor: &mut WorldEditor,
    set: &FacadeSet,
    cand: &Candidate,
    choice: Choice,
    scale: f64,
) -> usize {
    let lay = match layout(editor, cand, scale) {
        Ok(lay) => lay,
        Err(why) => {
            if dump() {
                eprintln!("FACADEPLACE {} {} 0 {why}", cand.way_id, cand.run);
            }
            return 0;
        }
    };
    let Some((src, px_per_m)) = set.image(choice.entry) else {
        if dump() {
            eprintln!("FACADEPLACE {} {} 0 noimage", cand.way_id, cand.run);
        }
        return 0;
    };
    let entry = &set.entries()[choice.entry];
    let fit = Fit::new(entry, px_per_m, lay.wall_w_m, lay.wall_h_m, choice.phase_m);

    let mut hung = 0usize;
    for stretch in &lay.stretches {
        let base_y = cand.base_y + stretch.rows_hidden;
        let rows = cand.total_h - stretch.rows_hidden;
        let footprint = &lay.cells[stretch.range.clone()];
        let mut crop = |p0: i32, p1: i32, b0: i32, b1: i32| {
            let (x0, x1, y0, y1) = piece_metres(&lay, stretch, scale, (p0, p1), (b0, b1));
            let piece = fit.region(&src, x0, x1, y0, y1);
            if dump() {
                let covered: Vec<String> = footprint[p0 as usize..p1 as usize]
                    .iter()
                    .map(|c| format!("{},{}", c.0, c.1))
                    .collect();
                eprintln!(
                    "FACADEPIECE {} {} {} {p0} {p1} {b0} {b1} {} {} {} {}",
                    cand.way_id,
                    cand.run,
                    stretch.wall - cand.run * SUBRUNS,
                    base_y + b0,
                    base_y + b1,
                    if piece.is_some() { "ok" } else { "none" },
                    covered.join(" ")
                );
            }
            piece
        };
        hung += displays::hang_wall(
            editor,
            'b',
            cand.way_id,
            stretch.wall,
            footprint,
            lay.n,
            lay.step,
            base_y,
            rows,
            &mut crop,
        );
    }
    if dump() {
        eprintln!("FACADEPLACE {} {} {hung} ok", cand.way_id, cand.run);
    }
    hung
}

/// The pieces `place_candidate` would hang for `cand`, as the atlas estimate
/// wants them: each piece's quad in blocks and a name for the picture it will
/// carry, the entry and the source pixels the piece's gather reads at the
/// resolution asked about, under which two pieces that will hold the same
/// pixels count as one texture, the way the writer will treat them. A piece
/// that then produces no panel, because its picture will not decode or its
/// quad falls outside the world, only charges the estimate for a texture the
/// pack will not carry.
fn pending_pieces(
    editor: &mut WorldEditor,
    set: &FacadeSet,
    cand: &Candidate,
    choice: Choice,
    scale: f64,
    out: &mut Vec<displays::Pending>,
) {
    let Ok(lay) = layout(editor, cand, scale) else {
        return;
    };
    let entry = &set.entries()[choice.entry];
    // The fit's own resolution plays no part: a piece is named at the
    // resolution the ladder asks about.
    let fit = Fit::new(
        entry,
        set.px_per_m(),
        lay.wall_w_m,
        lay.wall_h_m,
        choice.phase_m,
    );
    for stretch in &lay.stretches {
        let base_y = cand.base_y + stretch.rows_hidden;
        let rows = cand.total_h - stretch.rows_hidden;
        let footprint = &lay.cells[stretch.range.clone()];
        for piece in displays::pieces(footprint, lay.n, lay.step, base_y, rows) {
            let (x0, x1, y0, y1) = piece_metres(
                &lay,
                stretch,
                scale,
                (piece.p0, piece.p1),
                (piece.b0, piece.b1),
            );
            // Named at whatever resolution the ladder asks about: the pixels
            // two pieces share at one resolution they can differ in at another,
            // because each gather rounds to its own source pixels.
            let entry_index = choice.entry;
            let picture = Box::new(move |px: u32| {
                use std::hash::Hasher;
                let mut h = fnv::FnvHasher::default();
                h.write_usize(entry_index);
                h.write_u64(fit.region_key_at(px_per_m(px, scale), x0, x1, y0, y1));
                h.finish()
            });
            out.push(displays::Pending {
                w: piece.quad.w,
                h: piece.quad.h,
                picture,
            });
        }
    }
}

/// Settles the resolution the pack will be written at before any pending run
/// is cropped, and brings the set to it, so every crop from here on is built
/// at the size the writer puts in the pack rather than at the requested
/// resolution and shrunk afterwards. On a city block the crops built at the
/// requested resolution were two thirds of the run's peak memory, and the
/// pack used under half of their pixels.
///
/// The estimate is the pieces every pending run would hang, sized the way
/// `displays::fit_pending` sizes the panels it already holds. Runs settled
/// before this under region eviction were cropped at the requested
/// resolution and are in the registry already, where the estimate counts
/// them by their pixels; the writer shrinks those the way it always has.
fn fit_resolution(editor: &mut WorldEditor) {
    let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(set) = r.set.clone() else {
        return;
    };
    let scale = r.scale;
    let mut pending: Vec<displays::Pending> = Vec::new();
    for cand in r.candidates.iter().flatten() {
        let choice = picture_of(&r.groups, cand);
        pending_pieces(editor, &set, cand, choice, scale, &mut pending);
    }
    drop(r);
    if pending.is_empty() {
        return;
    }
    let Some(fit) = displays::fit_pending(&pending) else {
        return;
    };
    set.set_px_per_m(px_per_m(fit.px, scale));
    println!(
        "  Preset facades: {} panels to crop at {} px per block ({} textures, {:.1} Mpx of the atlas expected)",
        pending.len(),
        fit.px,
        fit.textures,
        fit.area as f64 / 1e6
    );
}

/// Metres along the wall that the cells `p0..p1` of a run span, from the
/// run's left end: the quad's own extent (`quad_for`), which is the spread of
/// the cell centres along the wall plus half a step at each end.
fn span_m(positions: &[f64], p0: usize, p1: usize, step: f64, scale: f64) -> (f64, f64) {
    let left = positions[0] - step / 2.0;
    let x0 = (positions[p0] - step / 2.0 - left) / scale;
    let x1 = (positions[p1 - 1] + step / 2.0 - left) / scale;
    (x0, x1)
}

/// How many of the wall rows `base_y..base_y + total_h` at `(bx, bz)` stand
/// behind the building in front of it: everything up to the highest block one
/// or two cells out along `(ox, oz)`, which on a party wall is that
/// building's roof, parapet or eave. Only asked of a blocked run, where the
/// wall pass has said what is in front, so a tree or a lamp post cannot be
/// mistaken for a neighbour. Rows below the wall's own foot are not looked
/// at: the ground there is not a building.
fn rows_behind_neighbour(
    editor: &WorldEditor,
    bx: i32,
    bz: i32,
    ox: i32,
    oz: i32,
    base_y: i32,
    total_h: i32,
) -> i32 {
    for y in (base_y..base_y + total_h).rev() {
        if (1..=2).any(|d| {
            editor
                .get_block_absolute(bx + ox * d, y, bz + oz * d)
                .is_some()
        }) {
            return y + 1 - base_y;
        }
    }
    0
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
        let party = cand.party;
        let hung = place_candidate(editor, &set, &cand, choice, scale);
        let mut r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        r.stats.panels += hung;
        if hung == 0 {
            r.stats.dropped += 1;
            if party {
                r.stats.behind += 1;
            }
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

/// How many pending candidates face into another building.
#[cfg(test)]
fn collected_party_runs() -> usize {
    let r = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    r.candidates.iter().flatten().filter(|c| c.party).count()
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
    // Every candidate is known, so the resolution the pack will end at can be
    // settled now and the crops built at it.
    fit_resolution(editor);
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
            if dump() {
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
    /// What the status line gets: the two numbers that say whether it worked,
    /// on one line. The breakdown behind them is in [`std::fmt::Display`],
    /// which goes to the terminal.
    fn summary(&self) -> String {
        let s = self.stats;
        format!(
            "Preset facades: {} panels on {} buildings",
            s.panels, s.buildings
        )
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        // A run behind a building as tall as itself is meant to hang nothing;
        // only the rest is a crop that failed.
        write!(
            f,
            "  Preset facades: {} panels on {} of {} wall runs over {} buildings{parts} ({} behind the building next door, {} without a usable crop)",
            s.panels,
            s.placed,
            s.candidates,
            s.buildings,
            s.behind,
            s.dropped - s.behind
        )
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
        let runs = open_runs(&cells, &|_, _| false, &|_, _| false);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0.len(), 10);
        assert!(!runs[0].1, "nothing in front of it");
    }

    #[test]
    fn mapillary_wins_the_columns_it_has_and_the_presets_fill_the_rest() {
        let cells: Vec<(i32, i32)> = (0..20).map(|i| (i, 0)).collect();
        // A photograph covering the middle of the wall, as a corner building
        // shot from one street gets.
        let free = |_: i32, _: i32| false;
        let runs = open_runs(&cells, &|bx, _| (6..14).contains(&bx), &free);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, (0..6).map(|i| (i, 0)).collect::<Vec<_>>());
        assert_eq!(runs[1].0, (14..20).map(|i| (i, 0)).collect::<Vec<_>>());
        // Covered end to end, nothing is left for the presets.
        assert!(open_runs(&cells, &|_, _| true, &free).is_empty());
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

    /// A terrace's end house: the east wall stands against the neighbour, so
    /// a panel there would hang inside the neighbour. It becomes a run of its
    /// own, marked, and the corner cells it shares with the north and south
    /// walls stay in those walls' free runs, because the question is asked
    /// per wall direction.
    #[test]
    fn a_wall_facing_another_building_is_a_party_run_of_its_own() {
        let ring = square();
        let neighbour_to_the_east =
            |bx: i32, _bz: i32, step: (i32, i32)| bx == 10 && step == (1, 0);
        let free = ring_runs(&ring, 1, None, &|_, _, _| false);
        assert_eq!(free.len(), 4, "every wall of a free-standing house");
        assert!(free.iter().all(|r| !r.party));
        let runs = ring_runs(&ring, 1, None, &neighbour_to_the_east);
        assert_eq!(runs.len(), 4);
        let east = runs
            .iter()
            .find(|r| r.outward == (10, 0))
            .expect("the east wall is still a run");
        assert!(east.party, "and it is the one that faces the neighbour");
        assert_eq!(east.cells.len(), 11);
        assert!(
            runs.iter().filter(|r| r.party).count() == 1,
            "no other wall faces anything"
        );
        let north = runs
            .iter()
            .find(|r| r.outward == (0, -10))
            .expect("the north wall is free");
        assert!(!north.party);
        assert_eq!(
            north.cells.len(),
            11,
            "the corner cell (10, 0) is still the north wall's"
        );
        assert!(north.cells.contains(&(10, 0)));
        // A neighbour that reaches only half way along: the free half and
        // the party half are two runs, the same shape `open_runs` gives a
        // wall Mapillary half covers.
        let half = |bx: i32, bz: i32, step: (i32, i32)| bx == 10 && step == (1, 0) && bz >= 5;
        let runs = ring_runs(&ring, 1, None, &half);
        let east: Vec<&Run> = runs.iter().filter(|r| r.outward == (10, 0)).collect();
        assert_eq!(east.len(), 2);
        assert_eq!(east[0].cells, (0..5).map(|z| (10, z)).collect::<Vec<_>>());
        assert!(!east[0].party);
        assert_eq!(east[1].cells, (5..11).map(|z| (10, z)).collect::<Vec<_>>());
        assert!(east[1].party);
    }

    /// The columns of a party wall are flattened like any other the presets
    /// may hang on, and recorded as a candidate that knows what it faces.
    #[test]
    fn collect_records_a_party_wall_as_a_party_candidate() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = set_dir();
        let xz =
            crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(0, 0, 200, 200).unwrap();
        reset(true, Some(dir.path()), 16, 1.0);
        let mut editor = crate::element_processing::building_test_support::test_editor(&xz);
        editor.set_map_decals(true);
        let ring = part_ring(20, 20, 20);
        let neighbour_to_the_east =
            |bx: i32, _bz: i32, step: (i32, i32)| bx == 40 && step == (1, 0);
        let shell = collect_facing(
            &mut editor,
            &ring,
            901,
            901,
            BuildingCategory::Default,
            0,
            0,
            12,
            &neighbour_to_the_east,
        );
        assert!(shell.contains(&(30, 20)) && shell.contains(&(40, 30)));
        assert_eq!(collected_pictures().len(), 4, "four walls, four runs");
        assert_eq!(collected_party_runs(), 1, "one of them faces the neighbour");

        // The same call with nobody next door.
        reset(true, Some(dir.path()), 16, 1.0);
        collect_facing(
            &mut editor,
            &ring,
            902,
            902,
            BuildingCategory::Default,
            0,
            0,
            12,
            &|_, _, _| false,
        );
        assert_eq!(collected_pictures().len(), 4);
        assert_eq!(collected_party_runs(), 0);
        reset(false, None, 16, 1.0);
    }

    /// Hangs the square house at (20, 20) with a neighbour along its east
    /// wall, filling x 41..=42 up to `roof(z)` for each z of the wall, and
    /// reads the panels back from the entities as (name, centre y, height).
    /// Wall rows are 1..=12.
    fn hang_beside(dir: &std::path::Path, roof: &dyn Fn(i32) -> i32) -> Vec<(String, f64, f64)> {
        let xz =
            crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(0, 0, 200, 200).unwrap();
        let ring = part_ring(20, 20, 20);
        let neighbour_to_the_east =
            |bx: i32, _bz: i32, step: (i32, i32)| bx == 40 && step == (1, 0);
        reset(true, Some(dir), 16, 1.0);
        displays::reset(true, 16);
        let mut editor = crate::element_processing::building_test_support::test_editor(&xz);
        editor.set_map_decals(true);
        for x in 41..=42 {
            for z in 20..=40 {
                for y in 1..=roof(z) {
                    editor.set_block_absolute(crate::block_definitions::STONE, x, y, z, None, None);
                }
            }
        }
        collect_facing(
            &mut editor,
            &ring,
            901,
            901,
            BuildingCategory::Default,
            0,
            0,
            12,
            &neighbour_to_the_east,
        );
        finalize(&mut editor).expect("something was collected");
        let mut out: Vec<(String, f64, f64)> = editor
            .item_displays()
            .iter()
            .map(|e| {
                let name = match e.get("item") {
                    Some(fastnbt::Value::Compound(item)) => match item.get("components") {
                        Some(fastnbt::Value::Compound(c)) => match c.get("minecraft:item_model") {
                            Some(fastnbt::Value::String(m)) => m.clone(),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    },
                    _ => String::new(),
                };
                let cy = match e.get("Pos") {
                    Some(fastnbt::Value::List(p)) => match p.get(1) {
                        Some(fastnbt::Value::Double(y)) => *y,
                        _ => f64::NAN,
                    },
                    _ => f64::NAN,
                };
                let h = match e.get("transformation") {
                    Some(fastnbt::Value::Compound(t)) => match t.get("scale") {
                        Some(fastnbt::Value::List(sc)) => match sc.get(1) {
                            Some(fastnbt::Value::Float(h)) => f64::from(*h),
                            _ => f64::NAN,
                        },
                        _ => f64::NAN,
                    },
                    _ => f64::NAN,
                };
                (name, cy, h)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        reset(false, None, 16, 1.0);
        displays::reset(false, 16);
        out
    }

    /// The east wall is run 1 of the ring (north, east, south, west), and a
    /// run's panels are named by `run * SUBRUNS + stretch`.
    fn is_east(name: &str) -> bool {
        name.strip_prefix("arnis:b901_")
            .and_then(|rest| rest.split('_').next())
            .and_then(|wall| wall.parse::<u32>().ok())
            .is_some_and(|wall| wall / SUBRUNS == 1)
    }

    /// The panel a party wall gets, read back from the entities: none where
    /// the neighbour is as tall, and only the storeys above its roof where it
    /// is lower, with the free walls hung top to bottom either way.
    #[test]
    fn a_party_wall_hangs_only_above_the_neighbours_roof() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = set_dir_with_pictures();

        // A neighbour as tall as the wall: three panels, none on the east.
        let panels = hang_beside(dir.path(), &|_| 12);
        assert_eq!(panels.len(), 3, "{panels:?}");
        assert!(panels.iter().all(|(n, _, _)| !is_east(n)), "{panels:?}");
        for (_, cy, h) in &panels {
            assert!(
                (h - 12.0).abs() < 1e-6 && (cy - 7.0).abs() < 1e-6,
                "{panels:?}"
            );
        }

        // A neighbour six rows tall: the east wall is hung from row 7 up.
        let panels = hang_beside(dir.path(), &|_| 6);
        assert_eq!(panels.len(), 4, "{panels:?}");
        let (_, cy, h) = panels
            .iter()
            .find(|(n, _, _)| is_east(n))
            .expect("the storeys above the neighbour get their picture");
        assert!(
            (h - 6.0).abs() < 1e-6,
            "six rows above a six row neighbour: {panels:?}"
        );
        assert!(
            (cy - 10.0).abs() < 1e-6,
            "centred on rows 7 to 12: {panels:?}"
        );
    }

    /// Two neighbours of different heights along one wall: a piece above each,
    /// and a pitched roof stepping up along the wall is one piece from its
    /// ridge rather than a one-cell sliver per step.
    #[test]
    fn a_party_wall_gets_a_piece_above_each_neighbour_and_one_over_a_pitched_roof() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = set_dir_with_pictures();

        // A four row house on the south half, an eight row one on the north.
        let panels = hang_beside(dir.path(), &|z| if z < 30 { 8 } else { 4 });
        let mut east: Vec<(f64, f64)> = panels
            .iter()
            .filter(|(n, _, _)| is_east(n))
            .map(|(_, cy, h)| (*cy, *h))
            .collect();
        east.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(
            east,
            vec![(11.0, 4.0), (9.0, 8.0)],
            "rows 9 to 12 over the tall house, rows 5 to 12 over the low one: {panels:?}"
        );

        // A roof rising a row every seven cells: hidden 5, 6 and 7 along the
        // wall, within `ROOF_STEP` of each other, so one piece from row 8 up.
        let panels = hang_beside(dir.path(), &|z| 5 + (z - 20) / 7);
        let east: Vec<(f64, f64)> = panels
            .iter()
            .filter(|(n, _, _)| is_east(n))
            .map(|(_, cy, h)| (*cy, *h))
            .collect();
        assert_eq!(east, vec![(10.5, 5.0)], "{panels:?}");
    }

    /// A piece of an oblique wall reads the metres its quad spans, so the
    /// pieces tile the wall exactly and the picture is never stretched to
    /// reach the quad's edge: a Bresenham walk can leave its last cell up to
    /// a minor step short of where the cell count says.
    #[test]
    fn a_piece_reads_the_metres_its_quad_spans() {
        let dir = (7, 3);
        let cells: Vec<(i32, i32)> = bresenham_line(0, 0, 0, 7, 0, 3)
            .into_iter()
            .map(|(x, _, z)| (x, z))
            .collect();
        let n = outward_normal(dir, 0, -1).unwrap();
        let (rx, rz) = right_of(n);
        let mut sorted = cells.clone();
        sorted.sort_by(|a, b| {
            let along = |c: &(i32, i32)| f64::from(c.0) * rx + f64::from(c.1) * rz;
            along(a).total_cmp(&along(b))
        });
        let positions: Vec<f64> = sorted
            .iter()
            .map(|c| f64::from(c.0) * rx + f64::from(c.1) * rz)
            .collect();
        let step = cell_step(dir);
        let quad = displays::quad_for(&sorted, n, step, 0, 1);
        let (x0, x1) = span_m(&positions, 0, positions.len(), step, 1.0);
        assert!(x0.abs() < 1e-9);
        assert!(
            (x1 - quad.w).abs() < 1e-9,
            "{x1} against the quad's {}",
            quad.w
        );
        // Each piece is exactly its own quad, from the run's left end to its
        // right end. Two neighbouring quads share or skip a sliver of less
        // than a step where the walk's cells sit closer or further apart than
        // the step, and the crops share or skip the same sliver, so the
        // pictures line up across the seam instead of one being stretched to
        // hide it. The count would have said four steps for the left piece;
        // the cells it actually holds span less.
        let (a0, a1) = span_m(&positions, 0, 4, step, 1.0);
        let (b0, b1) = span_m(&positions, 4, positions.len(), step, 1.0);
        assert!(a0.abs() < 1e-9 && (b1 - x1).abs() < 1e-9);
        assert!((a1 - b0).abs() < step, "{a1} against {b0}");
        let left = displays::quad_for(&sorted[..4], n, step, 0, 1);
        let right = displays::quad_for(&sorted[4..], n, step, 0, 1);
        assert!((a1 - a0 - left.w).abs() < 1e-9);
        assert!((b1 - b0 - right.w).abs() < 1e-9);
        assert!(
            (a1 - a0 - 4.0 * step).abs() > 0.05,
            "the count would have said {}, the piece's quad is {}",
            4.0 * step,
            a1 - a0
        );
    }

    #[test]
    fn a_run_of_one_cell_is_dropped() {
        // A sliver between two photographed stretches reads as a patch, so it
        // is left to the blocks.
        let free = |_: i32, _: i32| false;
        assert!(open_runs(&[(0, 0)], &free, &free).is_empty());
        assert_eq!(open_runs(&[(0, 0), (1, 0)], &free, &free).len(), 1);
        let cells: Vec<(i32, i32)> = (0..10).map(|i| (i, 0)).collect();
        assert_eq!(open_runs(&cells, &|bx, _| bx == 1, &free).len(), 1);
        // A party stretch of one cell is a sliver too.
        let runs = open_runs(&cells, &free, &|bx, _| bx == 4);
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|(_, party)| !party));
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
        set_dir_with(false)
    }

    /// The same set with a real picture behind every entry, for a test that
    /// hangs panels and so has to get past the decoder.
    fn set_dir_with_pictures() -> tempfile::TempDir {
        set_dir_with(true)
    }

    fn set_dir_with(pictures: bool) -> tempfile::TempDir {
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
            if pictures {
                image::RgbImage::from_pixel(32, 32, image::Rgb([180, 120, 90]))
                    .save(dir.path().join(&file))
                    .unwrap();
            } else {
                std::fs::write(dir.path().join(&file), b"").unwrap();
            }
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
        collect_facing(
            editor,
            &part_ring(x, z, 20),
            id,
            group_seed,
            BuildingCategory::Default,
            0,
            0,
            height,
            &|_, _, _| false,
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
    fn the_pixels_per_metre_follow_the_world_scale() {
        // Exactly the pack's pixels per block, whatever the scale: a crop
        // gathered at this is already the size the writer stores.
        for (px, scale, want) in [
            (16u32, 1.0f64, 16.0f64),
            (8, 2.0, 16.0),
            (11, 2.0, 22.0),
            // The set is 32 px per metre; finer than that is the writer's job.
            (32, 4.0, 32.0),
            (16, 4.0, 32.0),
            (4, 0.25, 1.0),
        ] {
            assert!(
                (px_per_m(px, scale) - want).abs() < 1e-9,
                "{px} px per block at {scale} blocks per metre"
            );
        }
    }

    /// The atlas is sized from the pending runs' geometry before any of them
    /// is cropped, and the set is brought to that resolution: the crops then
    /// come out at the size the pack stores, and the pack lands where the
    /// estimate said.
    #[test]
    fn the_crops_are_built_at_the_resolution_the_pack_ends_at() {
        let _guard = facades::TEST_GLOBALS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = set_dir_with_pictures();
        let xz = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(0, 0, 2000, 2000)
            .unwrap();
        // A budget nothing here can fill: the set stays at 16 px per block.
        reset(true, Some(dir.path()), 16, 2.0);
        displays::reset(true, 16);
        let mut editor = crate::element_processing::building_test_support::test_editor(&xz);
        editor.set_map_decals(true);
        collect_part(&mut editor, 901, 901, 20, 20, 12);
        let set = REGISTRY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set
            .clone()
            .unwrap();
        assert!((set.px_per_m() - 32.0).abs() < 1e-9, "16 px at scale 2");
        finalize(&mut editor).unwrap();
        assert!((set.px_per_m() - 32.0).abs() < 1e-9);
        let sizes = displays::panel_sizes_for_test();
        assert_eq!(sizes.len(), 4, "{sizes:?}");
        for (name, w, h, tw, th) in &sizes {
            assert_eq!(
                (*tw, *th),
                ((w * 16.0).round() as u32, (h * 16.0).round() as u32),
                "{name}: a {w} by {h} block quad cropped at 16 px"
            );
        }

        // Enough buildings that 16 px would overflow the atlas: the estimate
        // lowers the resolution first, the set follows, and every crop is at
        // the lower resolution rather than shrunk to it afterwards.
        reset(true, Some(dir.path()), 16, 2.0);
        displays::reset(true, 16);
        let mut editor = crate::element_processing::building_test_support::test_editor(&xz);
        editor.set_map_decals(true);
        // Every building its own size, so no two of them read the same
        // metres of one picture and the estimate cannot fold them together.
        let mut id = 1000u64;
        for i in 0..24 {
            for j in 0..24 {
                let side = 8 + (i * 24 + j) % 30;
                let height = 12 + (i * 7 + j * 3) % 50;
                collect_facing(
                    &mut editor,
                    &part_ring(20 + i * 60, 20 + j * 60, side),
                    id,
                    id,
                    BuildingCategory::Default,
                    0,
                    0,
                    height,
                    &|_, _, _| false,
                );
                id += 1;
            }
        }
        let set = REGISTRY
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set
            .clone()
            .unwrap();
        finalize(&mut editor).unwrap();
        let px = displays::px_for_test();
        assert!(
            px < 16,
            "the atlas cannot hold {} walls at 16 px",
            24 * 24 * 4
        );
        assert!(
            (set.px_per_m() - f64::from(px) * 2.0).abs() < 1e-9,
            "the set is at the pack's {px} px per block, got {}",
            set.px_per_m()
        );
        let sizes = displays::panel_sizes_for_test();
        assert!(!sizes.is_empty());
        for (name, w, h, tw, th) in &sizes {
            assert_eq!(
                (*tw, *th),
                (
                    (w * f64::from(px)).round() as u32,
                    (h * f64::from(px)).round() as u32
                ),
                "{name}: a {w} by {h} block quad cropped at {px} px"
            );
        }
        reset(false, None, 16, 1.0);
        displays::reset(false, 16);
    }
}
