//! Buildings under construction: a concrete frame of floor slabs and columns round a
//! stair core, clad on its lower storeys, with rebar and formwork on the storey going up,
//! scaffolding up some of its faces and, on a big site, a tower crane.
//!
//! Every choice is keyed on the way id and world coordinates, so tiles agree at seams.

use fnv::FnvHashSet;

use crate::block_definitions::*;
use crate::element_processing::buildings::cached_prop_block;
use crate::floodfill_cache::CoordinateBitmap;
use crate::land_cover::coord_hash;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

/// Column spacing along the frame, in metres.
const COLUMN_SPACING_M: f64 = 6.0;
/// Least a storey is in blocks, so a slab, a storey of air and the next slab fit.
const MIN_STOREY: i32 = 3;
/// Footprints up to this many square metres with at most this many storeys are houses:
/// walls of block all round instead of a frame, and scaffolding on every face.
const HOUSE_M2: f64 = 160.0;
const HOUSE_STOREYS: i32 = 3;
/// Footprint in square metres and storeys from which a site works with its own tower crane.
const CRANE_M2: f64 = 600.0;
const CRANE_STOREYS: i32 = 4;
/// Highest frame the bundled crane's hook still clears, in blocks above its base.
const CRANE_CLEARANCE: i32 = 55;

/// What the finished building will be, which decides how its lower storeys are clad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cladding {
    /// Block walls with window openings still empty.
    Masonry,
    /// Glass between mullions.
    Curtain,
}

fn cladding(way: &ProcessedWay) -> Cladding {
    let future = way
        .tags
        .get("construction")
        .or_else(|| way.tags.get("construction:building"))
        .map(String::as_str);
    match future {
        Some("office" | "commercial" | "retail" | "hotel" | "hospital" | "university") => {
            Cladding::Curtain
        }
        _ => Cladding::Masonry,
    }
}

/// Planned and built storeys, one slab per storey above the ground slab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Progress {
    pub storey: i32,
    pub planned: i32,
    pub built: i32,
}

impl Progress {
    /// Somewhere between two fifths and almost all of the way up, by the way id.
    pub(crate) fn of(id: u64, height: i32, storey: i32) -> Self {
        let storey = storey.max(MIN_STOREY);
        let planned = (height / storey).max(1);
        let share = 0.4 + (hash(id, 1) % 46) as f64 / 100.0;
        let built = ((planned as f64 * share).round() as i32).clamp(1, planned);
        Progress {
            storey,
            planned,
            built,
        }
    }

    fn topped_out(&self) -> bool {
        self.built >= self.planned
    }
}

fn hash(id: u64, salt: u64) -> u64 {
    coord_hash(
        (id ^ salt.wrapping_mul(0x9E37_79B9)) as i32,
        (id >> 32) as i32 ^ salt as i32,
    )
}

/// The footprint, its outline and the grid its columns stand on.
struct Plan {
    cells: Vec<(i32, i32)>,
    set: FnvHashSet<(i32, i32)>,
    min: (i32, i32),
    span: i32,
}

impl Plan {
    fn new(cells: &[(i32, i32)], scale: f64) -> Self {
        let set: FnvHashSet<(i32, i32)> = cells.iter().copied().collect();
        let min = cells
            .iter()
            .fold((i32::MAX, i32::MAX), |m, &(x, z)| (m.0.min(x), m.1.min(z)));
        Plan {
            cells: cells.to_vec(),
            set,
            min,
            span: ((COLUMN_SPACING_M * scale).round() as i32).clamp(3, 8),
        }
    }

    fn inside(&self, x: i32, z: i32) -> bool {
        self.set.contains(&(x, z))
    }

    /// Outward steps of a cell on the outline, none inside.
    fn outward(&self, x: i32, z: i32) -> impl Iterator<Item = (i32, i32)> + '_ {
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter(move |&(dx, dz)| !self.inside(x + dx, z + dz))
    }

    fn edge(&self, x: i32, z: i32) -> bool {
        self.outward(x, z).next().is_some()
    }

    /// Where a column stands: the corners, every span along the outline, and a grid inside.
    fn column(&self, x: i32, z: i32) -> bool {
        let (gx, gz) = (x - self.min.0, z - self.min.1);
        if self.edge(x, z) {
            let out: Vec<(i32, i32)> = self.outward(x, z).collect();
            let corner = out
                .iter()
                .any(|a| out.iter().any(|b| a.0 * b.0 + a.1 * b.1 == 0));
            corner || (gx + gz).rem_euclid(self.span) == 0
        } else {
            gx.rem_euclid(self.span) == 0
                && gz.rem_euclid(self.span) == 0
                && !self.outline_near(x, z)
        }
    }

    /// A grid column right next to the outline would double one standing on it.
    fn outline_near(&self, x: i32, z: i32) -> bool {
        (-1..=1)
            .any(|dx| (-1..=1).any(|dz| self.edge(x + dx, z + dz) && self.inside(x + dx, z + dz)))
    }

    /// The cell nearest the middle of the footprint.
    fn middle(&self) -> (i32, i32) {
        let n = self.cells.len().max(1) as i64;
        let (sx, sz) = self
            .cells
            .iter()
            .fold((0i64, 0i64), |s, &(x, z)| (s.0 + x as i64, s.1 + z as i64));
        let (cx, cz) = ((sx / n) as i32, (sz / n) as i32);
        self.cells
            .iter()
            .copied()
            .min_by_key(|&(x, z)| (i64::from(x - cx)).pow(2) + (i64::from(z - cz)).pow(2))
            .unwrap_or((cx, cz))
    }
}

/// The stair and lift core: a square of walls round the middle, if it fits inside.
fn core(plan: &Plan, scale: f64) -> Option<(i32, i32, i32)> {
    let size = ((4.0 * scale).round() as i32).clamp(3, 6);
    let (mx, mz) = plan.middle();
    let (x0, z0) = (mx - size / 2, mz - size / 2);
    let fits = (x0 - 1..x0 + size + 1).all(|x| (z0 - 1..z0 + size + 1).all(|z| plan.inside(x, z)));
    fits.then_some((x0, z0, size))
}

/// Builds the frame of a building going up on `cells`, its ground slab at `base_y`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    cells: &[(i32, i32)],
    base_y: i32,
    progress: Progress,
    terrain: bool,
    scale: f64,
    footprints: &CoordinateBitmap,
    passages: &CoordinateBitmap,
    passage_height: i32,
) {
    let plan = Plan::new(cells, scale);
    let Progress {
        storey,
        planned,
        built,
    } = progress;
    let slab_y = |k: i32| base_y + k * storey;
    let area_m2 = cells.len() as f64 / (scale * scale);
    let house = area_m2 <= HOUSE_M2 && planned <= HOUSE_STOREYS;
    let id = way.id;
    let open = |x: i32, z: i32, y: i32| passages.contains(x, z) && y <= base_y + passage_height;

    // Foundation down to the ground, then a slab for every storey built.
    for &(x, z) in &plan.cells {
        if passages.contains(x, z) {
            continue;
        }
        if terrain {
            if let Some(ground) = editor.terrain_level(x, z) {
                for y in ground..base_y {
                    editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None);
                }
            }
        }
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base_y, z, None, None);
    }
    for k in 1..=built {
        for &(x, z) in &plan.cells {
            if !open(x, z, slab_y(k)) {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, slab_y(k), z, None, None);
            }
        }
    }

    let core = (!house).then(|| core(&plan, scale)).flatten();
    let in_core = |x: i32, z: i32| {
        core.is_some_and(|(x0, z0, s)| (x0..x0 + s).contains(&x) && (z0..z0 + s).contains(&z))
    };

    // Columns through every storey built.
    for &(x, z) in &plan.cells {
        if house || in_core(x, z) || !plan.column(x, z) {
            continue;
        }
        for y in base_y + 1..slab_y(built) {
            if !open(x, z, y) {
                editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None);
            }
        }
    }

    // The storey going up: columns part of the way with rebar out of their tops, and
    // formwork for the next slab on props over part of the deck. A house's walls rise in
    // courses laid a stretch at a time.
    let top = slab_y(built);
    if house && !progress.topped_out() {
        for &(x, z) in &plan.cells {
            if !plan.edge(x, z) {
                continue;
            }
            let stretch = (x + z).div_euclid(4);
            let rise = 1 + (coord_hash(stretch ^ id as i32, 0x51) % (storey as u64 - 1)) as i32;
            for y in top + 1..=top + rise {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, None);
            }
        }
    } else if !progress.topped_out() {
        let (dx, dz) = match hash(id, 2) % 4 {
            0 => (1, 0),
            1 => (-1, 0),
            2 => (0, 1),
            _ => (0, -1),
        };
        let reach = plan
            .cells
            .iter()
            .map(|&(x, z)| x * dx + z * dz)
            .fold((i32::MAX, i32::MIN), |r, v| (r.0.min(v), r.1.max(v)));
        let formed = reach.0 + (reach.1 - reach.0) * (20 + (hash(id, 3) % 30) as i32) / 100;
        for &(x, z) in &plan.cells {
            let column = !in_core(x, z) && plan.column(x, z);
            if column {
                let rise = 1 + (coord_hash(x ^ id as i32, z) % (storey as u64 - 1)) as i32;
                for y in top + 1..=top + rise {
                    editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None);
                }
                for y in top + rise + 1..=top + rise + 2 {
                    editor.set_block_absolute(IRON_BARS, x, y, z, None, None);
                }
            } else if x * dx + z * dz <= formed && !in_core(x, z) {
                editor.set_block_absolute(SPRUCE_PLANKS, x, top + storey, z, None, None);
                if (x - plan.min.0).rem_euclid(3) == 0 && (z - plan.min.1).rem_euclid(3) == 0 {
                    for y in top + 1..top + storey {
                        editor.set_block_absolute(SPRUCE_FENCE, x, y, z, None, None);
                    }
                }
            }
        }
    } else {
        // Topped out: a parapet round the roof slab.
        for &(x, z) in &plan.cells {
            if plan.edge(x, z) {
                editor.set_block_absolute(GRAY_CONCRETE, x, top + 1, z, None, None);
            }
        }
    }

    // The core leads the frame by a storey, with a doorway onto every slab.
    let core_top = if progress.topped_out() {
        top + MIN_STOREY
    } else {
        top + storey
    };
    if let Some((x0, z0, s)) = core {
        let door = x0 + s / 2;
        for x in x0..x0 + s {
            for z in z0..z0 + s {
                let wall = x == x0 || x == x0 + s - 1 || z == z0 || z == z0 + s - 1;
                if !wall {
                    continue;
                }
                for y in base_y + 1..=core_top {
                    let level = (y - base_y).rem_euclid(storey);
                    if z == z0 && x == door && (1..=2).contains(&level) {
                        continue;
                    }
                    editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None);
                }
            }
        }
    }

    // Cladding on the lower storeys, the last of them only part of the way round.
    let kind = cladding(way);
    let clad = if house {
        built
    } else {
        ((built - 1) as f64 * (hash(id, 4) % 70) as f64 / 100.0).round() as i32
    };
    let started = (hash(id, 5) % 360) as f64;
    let (cx, cz) = plan.middle();
    for k in 1..=clad {
        for &(x, z) in &plan.cells {
            if !plan.edge(x, z) {
                continue;
            }
            if k == clad && !house && clad < built {
                let angle = f64::from(z - cz).atan2(f64::from(x - cx)).to_degrees() + 180.0;
                if (angle - started).rem_euclid(360.0) > 200.0 {
                    continue;
                }
            }
            let along = x + z;
            for y in slab_y(k - 1) + 1..slab_y(k) {
                if open(x, z, y) {
                    continue;
                }
                let row = y - slab_y(k - 1);
                let block = match kind {
                    Cladding::Masonry => {
                        let window = along.rem_euclid(3) != 0 && row >= 2 && row < storey;
                        if window {
                            continue;
                        }
                        if house {
                            LIGHT_GRAY_CONCRETE
                        } else {
                            WHITE_TERRACOTTA
                        }
                    }
                    Cladding::Curtain => {
                        if along.rem_euclid(3) == 0 {
                            GRAY_CONCRETE
                        } else {
                            LIGHT_BLUE_STAINED_GLASS
                        }
                    }
                };
                editor.set_block_absolute(block, x, y, z, None, None);
            }
        }
    }

    // Scaffolding along part of one or two faces, as high as the work on them has got:
    // a house's walls, a frame's cladding.
    let faces: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let first = (hash(id, 6) % 4) as usize;
    let chosen = [faces[first], faces[(first + 2) % 4]];
    let count = if hash(id, 7).is_multiple_of(2) { 2 } else { 1 };
    let scaffold = cached_prop_block(
        SCAFFOLDING,
        &[
            ("distance", "0"),
            ("bottom", "false"),
            ("waterlogged", "false"),
        ],
    );
    let scaffold_top = if house {
        top + if progress.topped_out() { 1 } else { 2 }
    } else {
        (slab_y(clad) + storey + 1).min(top + 2)
    };
    let mut stood: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    for (f, &n) in chosen.iter().enumerate().take(count) {
        let t = (-n.1, n.0);
        let along = |x: i32, z: i32| x * t.0 + z * t.1;
        let face: Vec<(i32, i32)> = plan
            .cells
            .iter()
            .copied()
            .filter(|&(x, z)| !plan.inside(x + n.0, z + n.1))
            .collect();
        let Some(lo) = face.iter().map(|&(x, z)| along(x, z)).min() else {
            continue;
        };
        let hi = face.iter().map(|&(x, z)| along(x, z)).max().unwrap_or(lo);
        let len = hi - lo + 1;
        let run = (len * (40 + (hash(id, 10 + f as u64) % 41) as i32) / 100).max(3);
        let start = lo + (hash(id, 20 + f as u64) % (len - run + 1).max(1) as u64) as i32;
        for &(x, z) in &face {
            if !(start..start + run).contains(&along(x, z)) {
                continue;
            }
            let (sx, sz) = (x + n.0, z + n.1);
            // Off roads, paths and parking, where cars and lamps come later.
            if footprints.contains(sx, sz)
                || passages.contains(sx, sz)
                || editor.surface_is_sealed(sx, sz)
                || editor.is_lc_water(sx, sz)
                || !stood.insert((sx, sz))
            {
                continue;
            }
            let ground = editor.get_absolute_y(sx, 0, sz);
            for y in ground + 1..=scaffold_top {
                editor.set_block_with_properties_absolute(scaffold.clone(), sx, y, sz, None, None);
            }
        }
    }

    // A crane mapped on the building is skipped as one inside a hall, so it stands here.
    let mapped = crate::structures::crane::mapped_crane_on(editor.mapped_cranes(), cells);
    let own = (!house
        && area_m2 >= CRANE_M2
        && planned >= CRANE_STOREYS
        && hash(id, 8).is_multiple_of(2))
    .then(|| {
        let (mx, mz) = plan.middle();
        core.map_or((mx, mz), |(x0, z0, s)| (x0 + s, z0 + s))
    })
    .filter(|&(x, z)| plan.inside(x, z));
    if top - base_y <= CRANE_CLEARANCE {
        if let Some((x, z)) = mapped.or(own) {
            crate::structures::crane::place_on_site(editor, x, z, base_y + 1, hash(id, 9));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{rect_way, test_editor};

    fn frame(id: u64, w: i32, l: i32, height: i32, tags: &[(&str, &str)]) -> WorldEditor<'static> {
        frame_on(id, w, l, height, tags, 1.0, |_| {})
    }

    /// A frame on an editor `prepare` has set up first.
    fn frame_on(
        id: u64,
        w: i32,
        l: i32,
        height: i32,
        tags: &[(&str, &str)],
        scale: f64,
        prepare: impl FnOnce(&mut WorldEditor),
    ) -> WorldEditor<'static> {
        let xzbbox = Box::leak(Box::new(
            XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap(),
        ));
        let mut editor = test_editor(xzbbox);
        prepare(&mut editor);
        let way = rect_way(id, 20, 20, 20 + w - 1, 20 + l - 1, tags);
        let cells: Vec<(i32, i32)> = (20..20 + w)
            .flat_map(|x| (20..20 + l).map(move |z| (x, z)))
            .collect();
        let empty = CoordinateBitmap::new_empty();
        generate(
            &mut editor,
            &way,
            &cells,
            0,
            Progress::of(id, height, 4),
            false,
            scale,
            &empty,
            &empty,
            4,
        );
        editor
    }

    #[test]
    fn progress_stops_short_of_the_plan_and_never_below_a_storey() {
        for id in 0..200u64 {
            let p = Progress::of(id, 60, 4);
            assert_eq!(p.planned, 15);
            assert!((1..=15).contains(&p.built));
            assert!(p.built >= 6, "{p:?}");
        }
        assert_eq!(Progress::of(7, 2, 4).built, 1);
    }

    #[test]
    fn a_frame_has_a_slab_every_storey_and_columns_between_but_open_storeys() {
        let id = 11;
        let p = Progress::of(id, 48, 4);
        let editor = frame(id, 30, 24, 48, &[("building", "construction")]);
        for k in 0..=p.built {
            assert!(editor.check_for_block_absolute(
                32,
                k * 4,
                30,
                Some(&[LIGHT_GRAY_CONCRETE]),
                None
            ));
        }
        // A corner column runs the whole frame; the middle of a bay is open.
        assert!(editor.check_for_block_absolute(20, 2, 20, Some(&[GRAY_CONCRETE]), None));
        assert!(!editor.block_exists_absolute(23, 6, 23));
        // Nothing stands on the deck above the frame's top storey but columns, rebar,
        // props, the core and scaffolding.
        let top = p.built * 4;
        assert!(editor.check_for_block_absolute(20, top + 1, 20, Some(&[GRAY_CONCRETE]), None));
    }

    #[test]
    fn scaffolding_stands_outside_the_footprint_on_the_ground() {
        let editor = frame(5, 30, 24, 48, &[("building", "construction")]);
        let mut found = 0;
        for x in 18..=51 {
            for z in 18..=45 {
                let inside = (20..50).contains(&x) && (20..44).contains(&z);
                if editor.check_for_block_absolute(x, 1, z, Some(&[SCAFFOLDING]), None) {
                    assert!(!inside, "scaffolding inside the footprint at ({x}, {z})");
                    found += 1;
                }
            }
        }
        assert!(found >= 10, "{found}");
    }

    #[test]
    fn scaffolding_keeps_off_roads_and_paths() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut sealed = CoordinateBitmap::new(&xzbbox);
        for x in 0..120 {
            for z in 0..120 {
                if !(20..50).contains(&x) || !(20..44).contains(&z) {
                    sealed.set(x, z);
                }
            }
        }
        let sealed = std::sync::Arc::new(sealed);
        let editor = frame_on(5, 30, 24, 48, &[("building", "construction")], 1.0, |e| {
            e.set_sealed_surface(sealed)
        });
        for x in 0..120 {
            for z in 0..120 {
                assert!(!editor.check_for_block_absolute(x, 1, z, Some(&[SCAFFOLDING]), None));
            }
        }
    }

    // place_mapped leaves out a crane on a building footprint, as one inside a hall.
    #[test]
    fn a_crane_mapped_on_the_frame_stands_where_it_is_mapped() {
        let id = (0..100u64)
            .find(|&id| !hash(id, 8).is_multiple_of(2))
            .unwrap();
        let p = Progress::of(id, 40, 4);
        let editor = frame_on(id, 30, 24, 40, &[("building", "construction")], 1.0, |e| {
            e.set_mapped_cranes(std::sync::Arc::new(vec![(24, 24)]))
        });
        let top = p.built * 4;
        let high = (20..50)
            .flat_map(|x| (20..44).map(move |z| (x, z)))
            .filter(|&(x, z)| editor.highest_block_between(x, z, top + 8, 300).is_some())
            .count();
        assert!(high > 0, "no crane over the frame");
        assert!(editor.highest_block_between(24, 24, top + 8, 300).is_some());
    }

    // Twice the blocks each way at scale 2 is still a 10 by 8 metre house, walled round an
    // empty inside rather than a frame of columns round a core.
    #[test]
    fn a_house_is_a_house_at_any_scale() {
        let tags = &[("building", "construction"), ("construction", "house")];
        let editor = frame_on(3, 20, 16, 8, tags, 2.0, |_| {});
        let inside = (22..38)
            .flat_map(|x| (22..34).map(move |z| (x, z)))
            .find(|&(x, z)| editor.block_exists_absolute(x, 1, z));
        assert_eq!(inside, None, "a column or core inside the house");
    }

    #[test]
    fn a_house_is_walled_all_round_and_an_office_is_glazed() {
        let house = frame(
            3,
            10,
            8,
            8,
            &[("building", "construction"), ("construction", "house")],
        );
        let walled = (20..30)
            .filter(|&x| house.block_exists_absolute(x, 1, 20))
            .count();
        assert_eq!(walled, 10, "the sill row is solid");
        // An office far enough along to have glass on.
        let id = (0..100u64)
            .find(|&id| {
                let p = Progress::of(id, 40, 4);
                ((p.built - 1) as f64 * (hash(id, 4) % 70) as f64 / 100.0).round() >= 2.0
            })
            .unwrap();
        let office = frame(
            id,
            30,
            24,
            40,
            &[("building", "construction"), ("construction", "office")],
        );
        let glass = |x: i32, y: i32, z: i32| {
            office.check_for_block_absolute(x, y, z, Some(&[LIGHT_BLUE_STAINED_GLASS]), None)
        };
        // The lowest storey is glazed all round, between mullions.
        assert!((21..49).any(|x| glass(x, 2, 20)) && (21..49).any(|x| glass(x, 2, 43)));
        assert!(!(21..49).any(|x| glass(x, 2, 30)), "no glass inside");
    }
}
