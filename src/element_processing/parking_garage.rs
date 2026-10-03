//! Multi-storey car parks: open decks with bay markings, a slab edge and parapet on
//! every level, columns, straight ramps between the decks, a stair tower up to the
//! roof, lights, and cars parked in some of the bays.
//!
//! Bays run across the footprint's longer side in modules of two bays back to back
//! with an aisle between, laid out from the footprint's bounding box.

use fnv::FnvHashSet;

use crate::block_definitions::*;
use crate::element_processing::buildings::cached_prop_block;
use crate::element_processing::connected_blocks::place_connected;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

/// Blocks from one deck to the next.
const LEVEL: i32 = 4;
/// Stripe to stripe across one bay, which fits the bundled cars.
const BAY_WIDTH: i32 = 6;
/// Bay depth, the length of a parked car and a block to spare.
const BAY_DEPTH: i32 = 9;
const AISLE: i32 = 6;
/// Two bays back to back with their aisle.
const MODULE: i32 = 2 * BAY_DEPTH + AISLE;
/// Columns stand on every third bay stripe.
const COLUMN_SPACING: i32 = 3 * BAY_WIDTH;
/// Outer columns along the facade.
const FACADE_COLUMN_SPACING: i32 = 6;
const RAMP_WIDTH: i32 = 4;
/// Ramp length: half a block of rise per block, one level in all.
const RAMP_RUN: i32 = 2 * LEVEL;

const SIDES: [(i32, i32); 4] = [(0, -1), (0, 1), (1, 0), (-1, 0)];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Facade {
    /// Bare concrete bands.
    Concrete,
    /// Concrete bands with a steel mesh screen between them.
    Mesh,
    /// Concrete bands with planting spilling over the parapet.
    Green,
    /// Brick parapets and columns.
    Brick,
}

/// Bounding-box frame: `a` runs along the longer side, `c` across it.
struct Frame {
    min_x: i32,
    min_z: i32,
    along_x: bool,
    len_a: i32,
    len_c: i32,
}

impl Frame {
    fn ac(&self, x: i32, z: i32) -> (i32, i32) {
        if self.along_x {
            (x - self.min_x, z - self.min_z)
        } else {
            (z - self.min_z, x - self.min_x)
        }
    }

    fn xz(&self, a: i32, c: i32) -> (i32, i32) {
        if self.along_x {
            (self.min_x + a, self.min_z + c)
        } else {
            (self.min_x + c, self.min_z + a)
        }
    }
}

/// A straight ramp `RAMP_WIDTH` rows wide from row `c0`, entered at `a0` and rising
/// over the following `RAMP_RUN` blocks onto the deck above.
#[derive(Clone, Copy, Debug)]
struct Ramp {
    a0: i32,
    c0: i32,
}

impl Ramp {
    /// Step along the ramp at (a, c): 1..=RAMP_RUN on the slope, 0 and RAMP_RUN + 1 at
    /// the entry and exit.
    fn step(&self, a: i32, c: i32) -> Option<i32> {
        let i = a - self.a0;
        ((0..=RAMP_RUN + 1).contains(&i) && (self.c0..self.c0 + RAMP_WIDTH).contains(&c))
            .then_some(i)
    }
}

/// A 3 by 3 stair tower from (x0, z0), its doorways on the `door` side.
#[derive(Clone, Copy, Debug)]
struct Core {
    x0: i32,
    z0: i32,
    door: (i32, i32),
}

impl Core {
    fn contains(&self, x: i32, z: i32) -> bool {
        (self.x0..self.x0 + 3).contains(&x) && (self.z0..self.z0 + 3).contains(&z)
    }

    fn centre(&self) -> (i32, i32) {
        (self.x0 + 1, self.z0 + 1)
    }
}

struct Plan {
    area: FnvHashSet<(i32, i32)>,
    edge: FnvHashSet<(i32, i32)>,
    frame: Frame,
    ramp: Option<Ramp>,
    core: Option<Core>,
}

impl Plan {
    fn interior(&self, x: i32, z: i32) -> bool {
        self.area.contains(&(x, z)) && !self.edge.contains(&(x, z))
    }

    fn ramp_step(&self, x: i32, z: i32) -> Option<i32> {
        let (a, c) = self.frame.ac(x, z);
        self.ramp.and_then(|r| r.step(a, c))
    }

    fn in_core(&self, x: i32, z: i32) -> bool {
        self.core.is_some_and(|core| core.contains(x, z))
    }

    /// Open deck: inside the slab edge, off the ramp and out of the stair tower.
    fn open_deck(&self, x: i32, z: i32) -> bool {
        self.interior(x, z) && self.ramp_step(x, z).is_none() && !self.in_core(x, z)
    }

    /// Position across a module: bays at 0..BAY_DEPTH and from BAY_DEPTH + AISLE.
    fn module_c(c: i32) -> i32 {
        (c - 1).rem_euclid(MODULE)
    }

    fn bay_stripe(&self, x: i32, z: i32) -> bool {
        let (a, c) = self.frame.ac(x, z);
        let m = Self::module_c(c);
        let in_bay = !(BAY_DEPTH..BAY_DEPTH + AISLE).contains(&m);
        in_bay && (a - 1).rem_euclid(BAY_WIDTH) == 0
    }

    /// Columns stand on bay stripes, at the back of the bays and either side of the aisle,
    /// but not right behind the facade.
    fn column(&self, x: i32, z: i32) -> bool {
        if SIDES.iter().any(|(dx, dz)| !self.interior(x + dx, z + dz)) {
            return false;
        }
        let (a, c) = self.frame.ac(x, z);
        let m = Self::module_c(c);
        (m == 0 || m == BAY_DEPTH || m == BAY_DEPTH + AISLE - 1)
            && (a - 1).rem_euclid(COLUMN_SPACING) == 0
    }

    /// Lights hang over the middle of the aisle, between the columns.
    fn light(&self, x: i32, z: i32) -> bool {
        let (a, c) = self.frame.ac(x, z);
        Self::module_c(c) == BAY_DEPTH + AISLE / 2
            && (a - 1).rem_euclid(COLUMN_SPACING) == COLUMN_SPACING / 2
    }

    /// Outer columns: at corners, and at a fixed spacing along each side.
    fn facade_column(&self, x: i32, z: i32) -> bool {
        let out = |dx: i32, dz: i32| !self.area.contains(&(x + dx, z + dz));
        let along_x_side = out(0, -1) || out(0, 1);
        let along_z_side = out(-1, 0) || out(1, 0);
        let on_x_grid = (x - self.frame.min_x).rem_euclid(FACADE_COLUMN_SPACING) == 0;
        let on_z_grid = (z - self.frame.min_z).rem_euclid(FACADE_COLUMN_SPACING) == 0;
        (along_x_side && (along_z_side || on_x_grid)) || (along_z_side && on_z_grid)
    }
}

fn find_ramp(
    area: &FnvHashSet<(i32, i32)>,
    edge: &FnvHashSet<(i32, i32)>,
    f: &Frame,
) -> Option<Ramp> {
    let fits = |ramp: Ramp| {
        (0..=RAMP_RUN + 1).all(|i| {
            (0..RAMP_WIDTH).all(|r| {
                let (x, z) = f.xz(ramp.a0 + i, ramp.c0 + r);
                area.contains(&(x, z)) && !edge.contains(&(x, z))
            })
        })
    };
    // Along one long side, else the other.
    for c0 in [1, f.len_c - RAMP_WIDTH] {
        for a0 in 1..f.len_a - RAMP_RUN - 1 {
            let ramp = Ramp { a0, c0 };
            if fits(ramp) {
                return Some(ramp);
            }
        }
    }
    None
}

fn find_core(plan: &Plan) -> Option<Core> {
    let f = &plan.frame;
    let (max_x, max_z) = if f.along_x {
        (f.min_x + f.len_a, f.min_z + f.len_c)
    } else {
        (f.min_x + f.len_c, f.min_z + f.len_a)
    };
    let (mid_x, mid_z) = ((f.min_x + max_x) / 2, (f.min_z + max_z) / 2);
    let corners = [
        (f.min_x, f.min_z, 1, 1),
        (max_x - 2, f.min_z, -1, 1),
        (f.min_x, max_z - 2, 1, -1),
        (max_x - 2, max_z - 2, -1, -1),
    ];
    // Every corner first, then a step further in.
    for t in 0..8 {
        for (cx, cz, sx, sz) in corners {
            let (x0, z0) = (cx + sx * t, cz + sz * t);
            let cells_ok = (0..3).all(|dx| {
                (0..3).all(|dz| {
                    let (x, z) = (x0 + dx, z0 + dz);
                    plan.area.contains(&(x, z)) && plan.ramp_step(x, z).is_none()
                })
            });
            if !cells_ok {
                continue;
            }
            // The doorway faces the middle of the deck, onto open deck.
            let (dx, dz) = (mid_x - (x0 + 1), mid_z - (z0 + 1));
            let door = if dx.abs() >= dz.abs() {
                (dx.signum(), 0)
            } else {
                (0, dz.signum())
            };
            if door == (0, 0) {
                continue;
            }
            let (ox, oz) = (x0 + 1 + 2 * door.0, z0 + 1 + 2 * door.1);
            if plan.open_deck(ox, oz) {
                return Some(Core { x0, z0, door });
            }
        }
    }
    None
}

fn facade_for(seed: u64) -> Facade {
    match seed % 20 {
        0..=7 => Facade::Concrete,
        8..=12 => Facade::Mesh,
        13..=16 => Facade::Green,
        _ => Facade::Brick,
    }
}

/// Builds a car park over `floor_area`, its ground deck at `base_y`.
pub fn generate_parking_garage(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    building_height: i32,
    base_y: i32,
    seed: u64,
) {
    if floor_area.is_empty() {
        return;
    }
    let area: FnvHashSet<(i32, i32)> = floor_area.iter().copied().collect();
    let edge: FnvHashSet<(i32, i32)> = floor_area
        .iter()
        .copied()
        .filter(|&(x, z)| {
            SIDES
                .iter()
                .any(|(dx, dz)| !area.contains(&(x + dx, z + dz)))
        })
        .collect();
    let (mut min_x, mut min_z, mut max_x, mut max_z) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(x, z) in floor_area {
        min_x = min_x.min(x);
        min_z = min_z.min(z);
        max_x = max_x.max(x);
        max_z = max_z.max(z);
    }
    let along_x = max_x - min_x >= max_z - min_z;
    let (len_a, len_c) = if along_x {
        (max_x - min_x, max_z - min_z)
    } else {
        (max_z - min_z, max_x - min_x)
    };
    let frame = Frame {
        min_x,
        min_z,
        along_x,
        len_a,
        len_c,
    };
    let ramp = find_ramp(&area, &edge, &frame);
    let mut plan = Plan {
        area,
        edge,
        frame,
        ramp,
        core: None,
    };
    plan.core = find_core(&plan);

    // The height spans the decks and the parapet over the top one, with at least one
    // deck above the ground. The stair tower and roof lamps rise above it, like the
    // rooftop equipment of other buildings.
    let decks = ((building_height - 2) / LEVEL).max(1);
    let top = base_y + decks * LEVEL;
    let facade = facade_for(seed);
    let (band, column) = if facade == Facade::Brick {
        (BRICK, BRICK)
    } else {
        (LIGHT_GRAY_CONCRETE, LIGHT_GRAY_CONCRETE)
    };

    build_foundation(editor, &plan, base_y);
    if let Some(core) = plan.core {
        build_core(editor, core, base_y, top);
    }
    for k in 0..=decks {
        build_deck(editor, &plan, base_y + k * LEVEL, k > 0);
    }
    if let Some(ramp) = plan.ramp {
        for k in 0..decks {
            build_ramp(editor, &plan, ramp, base_y + k * LEVEL);
        }
    }
    for k in 0..decks {
        build_level(
            editor,
            &plan,
            base_y + k * LEVEL,
            k,
            facade,
            band,
            column,
            seed,
        );
    }
    build_roof_deck(editor, &plan, top, band);
    park_cars(editor, &plan, base_y, decks);
}

/// Fills down to the terrain under the ground deck, so the car park does not float
/// where the ground falls away.
fn build_foundation(editor: &mut WorldEditor, plan: &Plan, base_y: i32) {
    for &(x, z) in &plan.area {
        let ground = editor.get_ground_level(x, z);
        let block = if plan.edge.contains(&(x, z)) {
            LIGHT_GRAY_CONCRETE
        } else {
            STONE
        };
        for y in ground..base_y {
            editor.set_block_absolute(block, x, y, z, None, None);
        }
    }
}

/// One deck: concrete slab edge, asphalt-grey driving surface, white bay stripes, and
/// an opening where the ramp from the deck below comes up.
fn build_deck(editor: &mut WorldEditor, plan: &Plan, y: i32, above_ground: bool) {
    for &(x, z) in &plan.area {
        if plan.in_core(x, z) {
            continue;
        }
        if above_ground
            && plan
                .ramp_step(x, z)
                .is_some_and(|i| (1..RAMP_RUN).contains(&i))
        {
            continue;
        }
        let block = if plan.edge.contains(&(x, z)) {
            LIGHT_GRAY_CONCRETE
        } else if plan.open_deck(x, z) && plan.bay_stripe(x, z) {
            WHITE_CONCRETE
        } else {
            GRAY_CONCRETE
        };
        editor.set_block_absolute(block, x, y, z, None, Some(&[]));
    }
}

/// The ramp from the deck at `y` to the one above, half a block of rise per block.
fn build_ramp(editor: &mut WorldEditor, plan: &Plan, ramp: Ramp, y: i32) {
    for i in 1..=RAMP_RUN {
        for r in 0..RAMP_WIDTH {
            let (x, z) = plan.frame.xz(ramp.a0 + i, ramp.c0 + r);
            if i % 2 == 0 {
                editor.set_block_absolute(SMOOTH_STONE, x, y + i / 2, z, None, Some(&[]));
            } else {
                editor.set_block_absolute(
                    SMOOTH_STONE_SLAB,
                    x,
                    y + (i + 1) / 2,
                    z,
                    None,
                    Some(&[]),
                );
            }
        }
    }
}

/// Columns, facade and lights of the level standing on the deck at `y`.
#[allow(clippy::too_many_arguments)]
fn build_level(
    editor: &mut WorldEditor,
    plan: &Plan,
    y: i32,
    level: i32,
    facade: Facade,
    band: Block,
    column: Block,
    seed: u64,
) {
    let lantern = cached_prop_block(LANTERN, &[("hanging", "true"), ("waterlogged", "false")]);
    for &(x, z) in &plan.area {
        if plan.in_core(x, z) {
            continue;
        }
        if plan.edge.contains(&(x, z)) {
            if plan.facade_column(x, z) {
                for dy in 1..LEVEL {
                    editor.set_block_absolute(column, x, y + dy, z, None, None);
                }
            } else if level > 0 {
                // The ground level stays open for the way in.
                editor.set_block_absolute(band, x, y + 1, z, None, None);
                match facade {
                    Facade::Mesh => {
                        for dy in 2..LEVEL {
                            place_connected(editor, IRON_BARS, x, y + dy, z);
                        }
                    }
                    Facade::Green => {
                        let h = coord_hash(x ^ (seed as i32), z ^ y);
                        let leaf = if h.is_multiple_of(3) {
                            AZALEA_LEAVES
                        } else {
                            OAK_LEAVES
                        };
                        if h % 100 < 60 {
                            editor.set_block_absolute(leaf, x, y + 2, z, None, None);
                            if (h >> 8) % 100 < 40 {
                                editor.set_block_absolute(leaf, x, y + 3, z, None, None);
                            }
                        }
                    }
                    Facade::Concrete | Facade::Brick => {}
                }
            }
        } else if plan.open_deck(x, z) && plan.column(x, z) {
            for dy in 1..LEVEL {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y + dy, z, None, None);
            }
        } else if plan.open_deck(x, z) && plan.light(x, z) {
            editor.set_block_with_properties_absolute(
                lantern.clone(),
                x,
                y + LEVEL - 1,
                z,
                None,
                None,
            );
        }
    }
}

/// The open top deck: a parapet all round and lamp posts along the aisles.
fn build_roof_deck(editor: &mut WorldEditor, plan: &Plan, y: i32, band: Block) {
    for &(x, z) in &plan.area {
        if plan.in_core(x, z) {
            continue;
        }
        if plan.edge.contains(&(x, z)) {
            editor.set_block_absolute(band, x, y + 1, z, None, None);
        } else if plan.open_deck(x, z) && plan.light(x, z) {
            editor.set_block_absolute(ANDESITE_WALL, x, y + 1, z, None, None);
            for dy in 2..=3 {
                editor.set_block_absolute(IRON_BARS, x, y + dy, z, None, None);
            }
            editor.set_block_absolute(SEA_LANTERN, x, y + 4, z, None, None);
            editor.set_block_absolute(SMOOTH_STONE_SLAB, x, y + 5, z, None, None);
        }
    }
}

/// A concrete stair tower from the ground deck to above the roof, with a doorway onto
/// every deck and a ladder up the inside.
fn build_core(editor: &mut WorldEditor, core: Core, base_y: i32, top: i32) {
    let (cx, cz) = core.centre();
    let (door_x, door_z) = (cx + core.door.0, cz + core.door.1);
    let facing = match core.door {
        (1, _) => "east",
        (-1, _) => "west",
        (_, 1) => "south",
        _ => "north",
    };
    let ladder = cached_prop_block(LADDER, &[("facing", facing), ("waterlogged", "false")]);
    for dx in 0..3 {
        for dz in 0..3 {
            let (x, z) = (core.x0 + dx, core.z0 + dz);
            editor.set_block_absolute(GRAY_CONCRETE, x, base_y, z, None, Some(&[]));
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, top + 4, z, None, Some(&[]));
            if (x, z) == (cx, cz) {
                for y in base_y + 1..=top + 3 {
                    editor.set_block_with_properties_absolute(
                        ladder.clone(),
                        x,
                        y,
                        z,
                        None,
                        Some(&[]),
                    );
                }
                continue;
            }
            for y in base_y + 1..=top + 3 {
                let deck_offset = (y - base_y).rem_euclid(LEVEL);
                let doorway = (x, z) == (door_x, door_z) && (deck_offset == 1 || deck_offset == 2);
                if !doorway {
                    editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, Some(&[]));
                }
            }
        }
    }
}

/// Cars in some of the bays on every deck: low ones under a deck, any on the roof.
fn park_cars(editor: &mut WorldEditor, plan: &Plan, base_y: i32, decks: i32) {
    let f = &plan.frame;
    let rot_base = if f.along_x { 0 } else { 1 };
    let bay_clear = |a0: i32, c0: i32| {
        (1..BAY_WIDTH).all(|da| {
            (0..BAY_DEPTH).all(|dc| {
                let (x, z) = f.xz(a0 + da, c0 + dc);
                plan.open_deck(x, z)
            })
        })
    };
    let mut bays: Vec<(i32, i32)> = Vec::new();
    for m in 0..=f.len_c / MODULE {
        for c0 in [1 + m * MODULE, 1 + m * MODULE + BAY_DEPTH + AISLE] {
            let mut a0 = 1;
            while a0 + BAY_WIDTH <= f.len_a {
                if bay_clear(a0, c0) {
                    bays.push(f.xz(a0 + BAY_WIDTH / 2, c0 + BAY_DEPTH / 2));
                }
                a0 += BAY_WIDTH;
            }
        }
    }
    for k in 0..=decks {
        let headroom = if k == decks { i32::MAX } else { LEVEL - 1 };
        let deck_top = base_y + k * LEVEL + 1;
        for &(x, z) in &bays {
            crate::structures::car::maybe_place_car_on_deck(
                editor, x, z, deck_top, rot_base, headroom,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::test_editor;

    fn rect_area(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
        (x0..=x1)
            .flat_map(|x| (z0..=z1).map(move |z| (x, z)))
            .collect()
    }

    fn build(area: &[(i32, i32)], height: i32, seed: u64) -> WorldEditor<'static> {
        let xzbbox = Box::leak(Box::new(
            XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap(),
        ));
        let mut editor = test_editor(xzbbox);
        generate_parking_garage(&mut editor, area, height, 0, seed);
        editor
    }

    fn block(editor: &WorldEditor, x: i32, y: i32, z: i32) -> Option<Block> {
        editor.get_block_absolute(x, y, z)
    }

    #[test]
    fn decks_stack_four_blocks_apart_with_open_sides_and_parapets() {
        let area = rect_area(10, 10, 70, 45);
        let editor = build(&area, 14, 0);
        // Three decks above the ground one at 14 blocks tall.
        for k in 0..=3 {
            assert!(block(&editor, 40, k * LEVEL, 30).is_some(), "deck {k}");
        }
        assert!(
            block(&editor, 40, 4 * LEVEL, 30).is_none(),
            "no fourth deck"
        );
        // A parapet band on the upper levels, open above it.
        assert_eq!(block(&editor, 13, LEVEL + 1, 10), Some(LIGHT_GRAY_CONCRETE));
        assert!(block(&editor, 13, LEVEL + 2, 10).is_none());
        // The ground level stays open between its columns.
        assert!(block(&editor, 13, 1, 10).is_none());
    }

    #[test]
    fn a_ramp_climbs_from_each_deck_to_the_next() {
        let area = rect_area(10, 10, 70, 45);
        let editor = build(&area, 14, 0);
        let plan_ramp = find_ramp(
            &area.iter().copied().collect(),
            &area
                .iter()
                .copied()
                .filter(|&(x, z)| x == 10 || x == 70 || z == 10 || z == 45)
                .collect(),
            &Frame {
                min_x: 10,
                min_z: 10,
                along_x: true,
                len_a: 60,
                len_c: 35,
            },
        )
        .expect("a ramp fits");
        let (x, z) = (10 + plan_ramp.a0, 10 + plan_ramp.c0);
        for k in 0..3 {
            let y = k * LEVEL;
            assert_eq!(block(&editor, x + 1, y + 1, z), Some(SMOOTH_STONE_SLAB));
            assert_eq!(block(&editor, x + 4, y + 2, z), Some(SMOOTH_STONE));
            assert_eq!(
                block(&editor, x + RAMP_RUN, y + LEVEL, z),
                Some(SMOOTH_STONE)
            );
            // Headroom over the low end of the ramp.
            for dy in 2..=4 {
                assert!(block(&editor, x + 1, y + dy, z).is_none(), "clear at {dy}");
            }
        }
    }

    #[test]
    fn a_stair_tower_climbs_above_the_roof() {
        let area = rect_area(10, 10, 70, 45);
        let editor = build(&area, 14, 0);
        let top = 3 * LEVEL;
        let ladders = (0..120)
            .flat_map(|x| (0..120).map(move |z| (x, z)))
            .filter(|&(x, z)| block(&editor, x, top + 3, z) == Some(LADDER))
            .count();
        assert_eq!(ladders, 1, "one ladder shaft reaching above the roof deck");
    }

    #[test]
    fn every_facade_style_is_drawn() {
        let area = rect_area(10, 10, 50, 40);
        let mut styles = std::collections::HashSet::new();
        for seed in 0..20u64 {
            let editor = build(&area, 10, seed);
            // A parapet cell on the north side, between two facade columns.
            let (x, z) = (25, 10);
            let facade = facade_for(seed);
            styles.insert(facade);
            let band = block(&editor, x, LEVEL + 1, z);
            let screen = block(&editor, x, LEVEL + 2, z);
            match facade {
                Facade::Brick => assert_eq!(band, Some(BRICK)),
                Facade::Mesh => assert_eq!(screen, Some(IRON_BARS)),
                Facade::Concrete => {
                    assert_eq!(band, Some(LIGHT_GRAY_CONCRETE));
                    assert!(screen.is_none());
                }
                Facade::Green => assert_eq!(band, Some(LIGHT_GRAY_CONCRETE)),
            }
        }
        assert_eq!(styles.len(), 4);
    }
}
