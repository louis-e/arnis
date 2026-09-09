//! Which photograph a building gets.
//!
//! Two things have to hold at once and they pull against each other.
//!
//! **Deterministic.** Two generations of the same area must agree, so nothing
//! here reads a random generator, a clock, a thread id or the order the tiles
//! happened to run in. The whole choice is a pure function of the building's
//! category, its height in metres, its position in the world and its OSM id.
//!
//! **Not the one next door.** A hash of the building id is uniform and
//! independent per building, which is exactly what makes it useless against
//! repetition: two neighbours drawing from the same shortlist of `k` collide
//! with probability `1/k` no matter how the hash is stirred, because
//! independence is the property being asked for and the property being
//! complained about. Coordination has to come from something the two
//! neighbours share, and the only thing they share without a pre-pass over the
//! whole world is **where they are**.
//!
//! So the shortlist is ranked by fit, and the pick inside it comes from a
//! lattice over the world grid, `cell_x + 2 * cell_z` on cells of
//! [`CELL_BLOCKS`], turned by a rotation hashed from the [`PATCH_CELLS`] by
//! [`PATCH_CELLS`] patch of cells the building is in.
//!
//! **What that does and does not promise.** Inside one patch two neighbours on
//! the same shortlist differ whenever the lattice step between their cells is
//! not a multiple of the shortlist length. A street laid out on a pitch that
//! is a multiple of that length is exactly the case a fixed lattice cannot
//! help with, and there is no cure: a colouring with `k` colours cannot
//! separate every pair at every spacing, and no lattice at all can separate
//! the eight neighbours of a cell with fewer than four colours, because they
//! form a king graph. The patch rotation is what keeps that from becoming a
//! whole city of one picture: a pitch that fails inside one patch gets a
//! different rotation in the next, so the damage is bounded by the patch.
//!
//! The constants are measured, not chosen by eye. Over street pitches from 8
//! to 30 blocks and shortlists of 3 to 8, this lattice repeats a picture
//! between neighbours **0.78 times as often as the building id hash on
//! average, and never more than 1.08 times as often** at the worst pitch and
//! shortlist; a 6-long shortlist goes from the hash's 18 per cent of
//! neighbouring pairs to 13, and an 8-long one from 12 to 9. Smaller cells,
//! larger cells and larger patches were all measured and are all worse. See
//! `lattice_is_never_much_worse_than_a_hash_and_usually_better`, which is that
//! measurement written as a test.
//!
//! What no lattice can do is separate two neighbours whose shortlists differ,
//! because it never sees the other building's list. That case is left at the
//! odds a hash gives it, on the grounds that two buildings of different
//! heights or different kinds sharing a photograph is the repetition nobody
//! notices.
//!
//! [`repetition_of`] measures all of it against the two alternatives, the
//! best-fit pick everyone writes first and the plain id hash; the tests at the
//! bottom of this file run it on a terraced street and on a mixed district and
//! print the numbers.

use super::manifest::{category_name, related, FacadeSet};
use crate::element_processing::buildings::BuildingCategory;

/// Storey height a photographed facade is expected to have, in metres. Used
/// only to rank: a manifest entry claiming 6 m storeys is either a mistake or
/// a warehouse, and either way it is the wrong picture for a block of flats.
const NOMINAL_STOREY_M: f64 = 3.1;

/// How much an odd storey height counts against an entry, against one unit of
/// log height error.
const STOREY_WEIGHT: f64 = 0.35;

/// How much it counts against an entry that the wall is not a whole number of
/// its storeys, so the crop cuts a window band in half rather than a floor.
const PARTIAL_STOREY_WEIGHT: f64 = 0.30;

/// How much it counts against an entry that it has no ground floor of its own,
/// which puts an upper storey window at street level.
const NO_GROUND_FLOOR_PENALTY: f64 = 0.15;

/// How much worse than the best fit an entry may be and still be shortlisted.
/// Wide enough that a category with several plausible pictures uses them all,
/// narrow enough that a four storey terrace never gets a tower.
const SHORTLIST_WINDOW: f64 = 0.45;

/// Longest shortlist. The repetition floor is one over this, so it is as long
/// as the fit window will allow rather than as short as looks tidy.
const SHORTLIST_MAX: usize = 8;

/// Side of the lattice cell in blocks. Measured: 6, 8, 10 and 12 were all
/// tried, and 8 came out best across street pitches from 8 to 30 blocks.
const CELL_BLOCKS: i32 = 8;

/// Side, in cells, of the patch whose hash rotates the lattice. Three was
/// measured against two and four: two rotates so often that the lattice barely
/// applies, four leaves a bad pitch bad over 32 blocks of street.
const PATCH_CELLS: i32 = 3;

/// What a building was given.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choice {
    /// Index into [`FacadeSet::entries`].
    pub entry: usize,
    /// Metres the tiling starts in from the texture's left edge.
    pub phase_m: f64,
}

/// splitmix64. Fixed, documented and stable across releases, which a hasher
/// from the standard library is not: the choice has to survive a toolchain
/// upgrade or two generations of one area stop agreeing.
pub fn hash64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash_pair(a: i64, b: i64) -> u64 {
    hash64(hash64(a as u64) ^ (b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// How badly `entry` suits a wall `wall_h_m` metres tall. Lower is better.
pub fn score(entry: &super::manifest::Entry, wall_h_m: f64) -> f64 {
    // Log ratio, so a 6 m wall on a 12 m photograph counts the same as a 24 m
    // wall on the same one: both are one doubling away from fitting.
    let height = (wall_h_m.max(0.5) / entry.metres_tall).ln().abs();
    let storey = (entry.storey_m - NOMINAL_STOREY_M).abs() / NOMINAL_STOREY_M;
    let storeys_on_wall = wall_h_m / entry.storey_m;
    let partial = (storeys_on_wall - storeys_on_wall.round()).abs();
    let ground = if entry.has_ground_floor {
        0.0
    } else {
        NO_GROUND_FLOOR_PENALTY
    };
    height + STOREY_WEIGHT * storey + PARTIAL_STOREY_WEIGHT * partial + ground
}

/// The entries a building of `category` may use, in the order the fallback
/// runs: its own category, then the categories next to it, then whatever the
/// set marks `Default`, then the whole set.
///
/// The last step is what makes a one-entry manifest work: every building gets
/// a facade, and a set that covers nothing in particular still covers
/// everything. The step that produced the list is returned for the summary.
pub fn candidates(set: &FacadeSet, category: BuildingCategory) -> (Vec<usize>, Fallback) {
    let own = set.in_category(category_name(category));
    if !own.is_empty() {
        return (own.to_vec(), Fallback::Own);
    }
    let mut near: Vec<usize> = Vec::new();
    for name in related(category) {
        for &index in set.in_category(name) {
            if !near.contains(&index) {
                near.push(index);
            }
        }
    }
    if !near.is_empty() {
        return (near, Fallback::Related);
    }
    let default = set.in_category(category_name(BuildingCategory::Default));
    if !default.is_empty() {
        return (default.to_vec(), Fallback::Default);
    }
    ((0..set.entries().len()).collect(), Fallback::WholeSet)
}

/// Which step of the fallback a building's shortlist came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    Own,
    Related,
    Default,
    WholeSet,
}

/// The shortlist for a building: the candidates that fit `wall_h_m` best,
/// ranked, cut to those within [`SHORTLIST_WINDOW`] of the best and at most
/// [`SHORTLIST_MAX`] long.
///
/// A pure function of the category and the height. It deliberately does not
/// depend on the building, so that two neighbours drawing from one shortlist
/// can be kept apart by where they are, and it does not depend on the width of
/// the individual wall, so that every wall of one building shows one building.
pub fn shortlist(set: &FacadeSet, category: BuildingCategory, wall_h_m: f64) -> Vec<usize> {
    let (mut list, _) = candidates(set, category);
    let entries = set.entries();
    // Ties broken by file name, not by anything about the building, so the
    // order is the same for every building that asks the same question.
    list.sort_by(|&a, &b| {
        score(&entries[a], wall_h_m)
            .total_cmp(&score(&entries[b], wall_h_m))
            .then_with(|| entries[a].file.cmp(&entries[b].file))
    });
    let Some(&best) = list.first() else {
        return list;
    };
    let cutoff = score(&entries[best], wall_h_m) + SHORTLIST_WINDOW;
    list.retain(|&i| score(&entries[i], wall_h_m) <= cutoff);
    list.truncate(SHORTLIST_MAX);
    list
}

/// Which of a shortlist of `k` a building at `anchor` takes. See the module
/// comment: a lattice over the world grid, turned by a per-district hash.
pub fn slot(anchor: (i32, i32), k: usize) -> usize {
    if k <= 1 {
        return 0;
    }
    let (cx, cz) = (
        anchor.0.div_euclid(CELL_BLOCKS),
        anchor.1.div_euclid(CELL_BLOCKS),
    );
    let lattice = i64::from(cx) + 2 * i64::from(cz);
    let patch = (cx.div_euclid(PATCH_CELLS), cz.div_euclid(PATCH_CELLS));
    let rot = (hash_pair(i64::from(patch.0), i64::from(patch.1)) % k as u64) as i64;
    (lattice + rot).rem_euclid(k as i64) as usize
}

/// The texture for a building of `category` whose walls are `wall_h_m` metres
/// tall, built under `way_id`, anchored at the block `anchor`.
///
/// `None` only for an empty set. Every wall of one building calls this with
/// the same arguments and so gets the same answer, without the walls having to
/// talk to each other across the tile threads.
///
/// This answers for one OSM element. A building drawn as several
/// `building:part` elements asks once per part and would get a picture each,
/// so `mod.rs` keeps the answer of the tallest part and hangs that on all of
/// them; see the module comment there.
pub fn choose(
    set: &FacadeSet,
    category: BuildingCategory,
    wall_h_m: f64,
    way_id: u64,
    anchor: (i32, i32),
) -> Option<Choice> {
    let list = shortlist(set, category, wall_h_m);
    let entry = *list.get(slot(anchor, list.len()))?;
    // The id decides only where along the texture the tiling starts. A phase
    // cannot make two neighbours share a picture, so this is the one place the
    // building's own id can vary the result without costing the lattice its
    // guarantee.
    let width = set.entries()[entry].metres_wide;
    let steps = 16u64;
    let phase_m = (hash64(way_id) % steps) as f64 / steps as f64 * width;
    Some(Choice { entry, phase_m })
}

/// How often the same texture lands on two buildings that touch, over a set of
/// buildings given as (id, anchor, category, height).
///
/// Three ways of choosing are measured side by side, because a repetition rate
/// on its own says nothing: `best` is the pick everyone writes first, always
/// the closest fit, which puts one picture on a whole terrace; `hash` is the
/// building id hash, which is uniform and independent and so cannot do better
/// than one in the shortlist's length; `lattice` is what this module does.
/// `radius` is how far apart two anchors may be and still count as neighbours,
/// in blocks.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Repetition {
    /// Neighbouring pairs examined.
    pub pairs: usize,
    /// Of those, the pairs whose two buildings share a shortlist, which is the
    /// only case a lattice can do anything about.
    pub same_shortlist: usize,
    pub best_same: usize,
    pub hash_same: usize,
    pub lattice_same: usize,
    /// Repeats among the pairs that share a shortlist.
    pub lattice_same_of_shared: usize,
}

#[cfg(test)]
impl Repetition {
    pub fn line(&self, what: &str) -> String {
        let pct = |n: usize| 100.0 * n as f64 / self.pairs.max(1) as f64;
        format!(
            "{what}: {} neighbour pairs ({} share a shortlist); same texture: best fit {} \
             ({:.1}%), id hash {} ({:.1}%), lattice {} ({:.1}%), and {} of the {} \
             shared-shortlist pairs",
            self.pairs,
            self.same_shortlist,
            self.best_same,
            pct(self.best_same),
            self.hash_same,
            pct(self.hash_same),
            self.lattice_same,
            pct(self.lattice_same),
            self.lattice_same_of_shared,
            self.same_shortlist
        )
    }
}

#[cfg(test)]
pub fn repetition_of(
    set: &FacadeSet,
    buildings: &[(u64, (i32, i32), BuildingCategory, f64)],
    radius: i32,
) -> Repetition {
    let mut out = Repetition::default();
    for (i, &(id_a, anchor_a, cat_a, h_a)) in buildings.iter().enumerate() {
        for &(id_b, anchor_b, cat_b, h_b) in &buildings[i + 1..] {
            let dx = anchor_a.0 - anchor_b.0;
            let dz = anchor_a.1 - anchor_b.1;
            if dx * dx + dz * dz > radius * radius {
                continue;
            }
            let list_a = shortlist(set, cat_a, h_a);
            let list_b = shortlist(set, cat_b, h_b);
            if list_a.is_empty() || list_b.is_empty() {
                continue;
            }
            out.pairs += 1;
            let shared = list_a == list_b;
            if shared {
                out.same_shortlist += 1;
            }
            if list_a[0] == list_b[0] {
                out.best_same += 1;
            }
            if list_a[(hash64(id_a) % list_a.len() as u64) as usize]
                == list_b[(hash64(id_b) % list_b.len() as u64) as usize]
            {
                out.hash_same += 1;
            }
            if list_a[slot(anchor_a, list_a.len())] == list_b[slot(anchor_b, list_b.len())] {
                out.lattice_same += 1;
                if shared {
                    out.lattice_same_of_shared += 1;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::building_facades::manifest;

    /// Eight residential pictures of different heights plus a shop and a
    /// catch-all, enough that a shortlist is really a list.
    const SET: &str = r#"{
      "version": 1,
      "textures": [
        {"file": "r03.png", "categories": ["Residential"], "metres_wide": 10.0,
         "metres_tall": 6.2, "storeys": 2, "tiles_horizontally": true, "has_ground_floor": true},
        {"file": "r04.png", "categories": ["Residential"], "metres_wide": 11.0,
         "metres_tall": 9.3, "storeys": 3, "tiles_horizontally": true, "has_ground_floor": true},
        {"file": "r05.png", "categories": ["Residential"], "metres_wide": 12.0,
         "metres_tall": 12.4, "storeys": 4, "tiles_horizontally": true, "has_ground_floor": true},
        {"file": "r06.png", "categories": ["Residential"], "metres_wide": 13.0,
         "metres_tall": 12.4, "storeys": 4, "tiles_horizontally": true, "has_ground_floor": true},
        {"file": "r07.png", "categories": ["Residential"], "metres_wide": 14.0,
         "metres_tall": 15.5, "storeys": 5, "tiles_horizontally": true, "has_ground_floor": true},
        {"file": "r08.png", "categories": ["Residential"], "metres_wide": 15.0,
         "metres_tall": 15.5, "storeys": 5, "tiles_horizontally": true, "has_ground_floor": true},
        {"file": "r09.png", "categories": ["Residential", "TallBuilding"], "metres_wide": 24.0,
         "metres_tall": 27.9, "storeys": 9, "tiles_horizontally": true, "has_ground_floor": false},
        {"file": "r10.png", "categories": ["Residential", "TallBuilding"], "metres_wide": 25.0,
         "metres_tall": 31.0, "storeys": 10, "tiles_horizontally": true, "has_ground_floor": false},
        {"file": "s01.png", "categories": ["Commercial"], "metres_wide": 8.0,
         "metres_tall": 6.2, "storeys": 2, "has_ground_floor": true},
        {"file": "d01.png", "categories": ["Default"], "metres_wide": 10.0,
         "metres_tall": 9.3, "storeys": 3, "tiles_horizontally": true, "has_ground_floor": true}
      ]
    }"#;

    fn sample_set() -> (FacadeSet, tempfile::TempDir) {
        let (set, _report, dir) = manifest::set_for_test(SET, 8.0);
        (set, dir)
    }

    #[test]
    fn the_choice_is_the_same_every_time() {
        let (set, _dir) = sample_set();
        let first = choose(&set, BuildingCategory::Residential, 12.4, 42, (100, -60)).unwrap();
        for _ in 0..50 {
            let again = choose(&set, BuildingCategory::Residential, 12.4, 42, (100, -60)).unwrap();
            assert_eq!(first, again);
        }
        // A second load of the same manifest is the same set, so a second run
        // of the generator hangs the same picture.
        let (reloaded, _dir2) = sample_set();
        assert_eq!(
            choose(
                &reloaded,
                BuildingCategory::Residential,
                12.4,
                42,
                (100, -60)
            )
            .unwrap(),
            first
        );
    }

    #[test]
    fn every_wall_of_one_building_gets_one_picture() {
        // The chooser is not given the wall's width, so a building's long side
        // and its short side cannot disagree about what building it is.
        let (set, _dir) = sample_set();
        let a = choose(&set, BuildingCategory::Residential, 15.5, 7, (12, 12)).unwrap();
        let b = choose(&set, BuildingCategory::Residential, 15.5, 7, (12, 12)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn the_shortlist_is_ranked_by_how_well_the_height_fits() {
        let (set, _dir) = sample_set();
        let short = shortlist(&set, BuildingCategory::Residential, 6.2);
        let tall = shortlist(&set, BuildingCategory::Residential, 27.9);
        assert_eq!(set.entries()[short[0]].file, "r03.png");
        assert_eq!(set.entries()[tall[0]].file, "r09.png");
        // A two storey wall is never given a nine storey photograph, whatever
        // the lattice says: the shortlist does not contain one.
        for &i in &short {
            assert!(
                set.entries()[i].metres_tall < 16.0,
                "{}",
                set.entries()[i].file
            );
        }
        assert!(!short.is_empty() && short.len() <= SHORTLIST_MAX);
    }

    #[test]
    fn a_category_with_no_texture_falls_back_and_never_fails() {
        let (set, _dir) = sample_set();
        // Nothing is tagged Warehouse, so the related categories are tried,
        // and nothing is tagged those either, so Default catches it.
        let (list, step) = candidates(&set, BuildingCategory::Warehouse);
        assert_eq!(step, Fallback::Default);
        assert_eq!(list.len(), 1);
        assert_eq!(set.entries()[list[0]].file, "d01.png");
        assert!(choose(&set, BuildingCategory::Warehouse, 8.0, 1, (0, 0)).is_some());

        // Hotel has no texture either, but Residential is next to it.
        let (list, step) = candidates(&set, BuildingCategory::Hotel);
        assert_eq!(step, Fallback::Related);
        assert!(list.len() >= 8);

        // Every category the generator can produce gets something.
        for category in [
            BuildingCategory::Residential,
            BuildingCategory::House,
            BuildingCategory::Farm,
            BuildingCategory::Commercial,
            BuildingCategory::Office,
            BuildingCategory::Hotel,
            BuildingCategory::Industrial,
            BuildingCategory::Warehouse,
            BuildingCategory::School,
            BuildingCategory::Hospital,
            BuildingCategory::Religious,
            BuildingCategory::TallBuilding,
            BuildingCategory::GlassySkyscraper,
            BuildingCategory::GlassCornerSkyscraper,
            BuildingCategory::GridSkyscraper,
            BuildingCategory::ContemporarySkyscraper,
            BuildingCategory::ModernSkyscraper,
            BuildingCategory::MasonrySkyscraper,
            BuildingCategory::Historic,
            BuildingCategory::Tower,
            BuildingCategory::Garage,
            BuildingCategory::Shed,
            BuildingCategory::Greenhouse,
            BuildingCategory::Default,
        ] {
            assert!(
                choose(&set, category, 10.0, 3, (0, 0)).is_some(),
                "{category:?}"
            );
        }
    }

    #[test]
    fn a_one_texture_set_still_dresses_every_building() {
        let one = r#"{"version": 1, "textures": [
          {"file": "only.png", "categories": ["Shed"], "metres_wide": 6.0,
           "metres_tall": 3.0, "storeys": 1, "has_ground_floor": true}]}"#;
        let (set, _report, _dir) = manifest::set_for_test(one, 8.0);
        let (list, step) = candidates(&set, BuildingCategory::GlassySkyscraper);
        assert_eq!(step, Fallback::WholeSet);
        assert_eq!(list, vec![0]);
        assert_eq!(
            choose(&set, BuildingCategory::Office, 40.0, 9, (5, 5))
                .unwrap()
                .entry,
            0
        );
    }

    /// The claim in the module comment, as a measurement: a grid of buildings
    /// all drawing on one shortlist, over the street pitches a real city is
    /// laid out on. What has to hold is that the lattice is never materially
    /// worse than the id hash it replaces, and clearly better on average.
    #[test]
    fn lattice_is_never_much_worse_than_a_hash_and_usually_better() {
        let mut ratio_sum = 0.0;
        let mut worst: f64 = 0.0;
        let mut cases = 0;
        for k in 3..=SHORTLIST_MAX {
            for pitch in [8i32, 9, 10, 11, 12, 13, 14, 16, 18, 20, 25, 30] {
                let mut pairs = 0usize;
                let mut same = 0usize;
                for ra in 0..20i32 {
                    for ca in 0..20i32 {
                        let a = (ca * pitch, ra * pitch);
                        // The four buildings around this one that have not
                        // already been counted from the other side.
                        for (dx, dz) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
                            let b = (a.0 + dx * pitch, a.1 + dz * pitch);
                            pairs += 1;
                            if slot(a, k) == slot(b, k) {
                                same += 1;
                            }
                        }
                    }
                }
                // A uniform independent hash lands on the same entry one time
                // in k, whatever the pitch; that is the bar.
                let ratio = (same as f64 / pairs as f64) * k as f64;
                ratio_sum += ratio;
                worst = worst.max(ratio);
                cases += 1;
            }
        }
        let average = ratio_sum / f64::from(cases);
        println!(
            "lattice against a uniform hash: {average:.2} times as many repeats on average, {worst:.2} at worst, over {cases} pitch and shortlist combinations"
        );
        assert!(
            average < 0.85,
            "no better than a hash on average: {average:.2}"
        );
        assert!(worst < 1.15, "much worse than a hash somewhere: {worst:.2}");
    }

    /// A terraced street: rows of buildings 12 m apart whose height and kind
    /// run along the row, the way a real terrace does, and the shape this
    /// feature is pointed at. Ids are sequential, as an OSM import makes them.
    fn terraced_street() -> Vec<(u64, (i32, i32), BuildingCategory, f64)> {
        let mut out = Vec::new();
        for row in 0..24i32 {
            // One height and one kind per row: a terrace is built at once.
            let storeys = 3.0 + (hash64(row as u64) % 4) as f64;
            let category = if hash64(row as u64 ^ 0x99).is_multiple_of(5) {
                BuildingCategory::Commercial
            } else {
                BuildingCategory::Residential
            };
            for col in 0..24i32 {
                let id = 100_000 + u64::from((row * 24 + col) as u32);
                out.push((id, (col * 12, row * 12), category, storeys * 3.1));
            }
        }
        out
    }

    /// The harder case: every building its own height and kind, so almost no
    /// two neighbours draw on the same shortlist and the lattice has nothing
    /// to work with. A city centre of one-off buildings, not a terrace.
    fn mixed_district() -> Vec<(u64, (i32, i32), BuildingCategory, f64)> {
        let mut out = Vec::new();
        for row in 0..24i32 {
            for col in 0..24i32 {
                let id = 100_000 + u64::from((row * 24 + col) as u32);
                let category = if hash64(id).is_multiple_of(8) {
                    BuildingCategory::Commercial
                } else {
                    BuildingCategory::Residential
                };
                let storeys = 3.0 + (hash64(id ^ 0x5555) % 4) as f64;
                out.push((id, (col * 12, row * 12), category, storeys * 3.1));
            }
        }
        out
    }

    /// The same two measurements against the set that is actually installed,
    /// which the tests above cannot use: the photographs are not in the
    /// repository (see `assets/building-facades/PROVENANCE.md`), so on a fresh
    /// clone there is nothing to measure and this would fail for the wrong
    /// reason. Ignored like the golden facade fixtures, and run by hand with
    /// `cargo test installed_set -- --ignored --nocapture` when the numbers in
    /// the design notes need checking against a real manifest.
    #[test]
    #[ignore = "needs the installed facade set, which is not in the repository"]
    fn repetition_on_the_installed_set() {
        let Some(dir) = manifest::resolve_dir(None) else {
            panic!("no facade set installed");
        };
        let (set, report) = manifest::load(&dir, 16.0).unwrap();
        println!("{} from {}", report.summary(), dir.display());
        for (what, buildings) in [
            ("terraced street", terraced_street()),
            ("mixed district", mixed_district()),
        ] {
            println!("{}", repetition_of(&set, &buildings, 17).line(what));
        }
    }

    #[test]
    fn a_terrace_repeats_far_less_than_the_best_fit_or_a_hash() {
        let (set, _dir) = sample_set();
        // 17 blocks reaches the four buildings orthogonally next door and the
        // four diagonally, on a 12 block grid.
        let m = repetition_of(&set, &terraced_street(), 17);
        // Printed so the numbers in the report can be reproduced by running
        // this one test with --nocapture.
        println!("{}", m.line("terraced street"));
        assert!(m.pairs > 2000, "{} neighbour pairs", m.pairs);
        // Always taking the best fit puts one picture on about half the
        // terrace, which is what the shortlist and the lattice are here to
        // undo.
        assert!(
            m.best_same * 5 > m.pairs * 2,
            "best fit repeated on {} of {} pairs",
            m.best_same,
            m.pairs
        );
        assert!(
            m.lattice_same * 4 < m.best_same * 3,
            "lattice {} against best fit {}",
            m.lattice_same,
            m.best_same
        );
        assert!(
            m.lattice_same < m.hash_same,
            "lattice {} against id hash {}",
            m.lattice_same,
            m.hash_same
        );
    }

    #[test]
    fn a_district_of_one_off_buildings_is_no_worse_than_a_hash() {
        let (set, _dir) = sample_set();
        let m = repetition_of(&set, &mixed_district(), 17);
        println!("{}", m.line("mixed district"));
        // Nothing can separate two buildings whose shortlists differ without
        // seeing the other list, so this case is left at the odds a hash gives
        // it. What must hold is that it is not made worse.
        assert!(
            m.lattice_same <= m.hash_same,
            "lattice {} against id hash {}",
            m.lattice_same,
            m.hash_same
        );
        assert!(m.lattice_same * 2 < m.best_same);
    }

    #[test]
    fn the_whole_set_is_used_and_not_just_the_best_fit() {
        // A chooser that always took the best fit would put one picture on a
        // whole street. Count how many distinct entries a district uses.
        let (set, _dir) = sample_set();
        let buildings = terraced_street();
        let mut used = std::collections::BTreeSet::new();
        for &(id, anchor, category, h) in &buildings {
            used.insert(choose(&set, category, h, id, anchor).unwrap().entry);
        }
        assert!(
            used.len() >= 6,
            "only {} distinct textures used",
            used.len()
        );
    }

    #[test]
    fn the_phase_varies_between_buildings_without_changing_the_picture() {
        let (set, _dir) = sample_set();
        let a = choose(&set, BuildingCategory::Residential, 12.4, 1, (0, 0)).unwrap();
        let mut phases = std::collections::BTreeSet::new();
        for id in 0..64u64 {
            let c = choose(&set, BuildingCategory::Residential, 12.4, id, (0, 0)).unwrap();
            assert_eq!(c.entry, a.entry, "the id must not change the picture");
            phases.insert(c.phase_m.to_bits());
        }
        assert!(phases.len() >= 8, "only {} distinct phases", phases.len());
    }
}
