//! Construction sites: churned ground in patches, a mesh fence around the edge, and a
//! site laid out in plots, each holding at most one thing: a heap of sand or spoil, a
//! stack of building material or timber, a foundation slab with rebar, a site cabin,
//! a container or a scaffold tower.
//!
//! Ground and plots are keyed on world coordinates, and every prop stays inside its own
//! plot, so props never collide and tiles agree at their seams.

use fnv::FnvHashSet;

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::climate::Climate;
use crate::element_processing::buildings::cached_prop_block;
use crate::element_processing::connected_blocks::{four_connected_line, place_connected};
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

/// Side of one plot. A prop keeps a one-block margin inside it.
const PLOT: i32 = 13;
/// Largest prop extent along either axis, so it fits the plot with its margin.
const MAX_EXTENT: i32 = PLOT - 2;

/// Ground blocks of a site, which props may stand on and a foundation may replace.
pub(crate) const SITE_GROUND: &[Block] = &[GRAVEL, COARSE_DIRT, DIRT, MUD];

/// Ground at (x, z): gravel haul areas, churned coarse dirt, loose soil and wet low
/// spots in patches a few blocks across, with a speckle of the neighbouring kind.
pub fn ground_block(x: i32, z: i32, arid: bool) -> Block {
    let n = value_noise_01(x + 311, z - 173, 11);
    let speck = coord_hash(x ^ 0x2C51, z) % 100;
    if n < 0.22 {
        if speck < 4 {
            COARSE_DIRT
        } else {
            GRAVEL
        }
    } else if n < 0.62 {
        if speck < 3 {
            GRAVEL
        } else {
            COARSE_DIRT
        }
    } else if n < 0.84 || arid {
        if speck < 6 {
            COARSE_DIRT
        } else {
            DIRT
        }
    } else if speck < 12 {
        DIRT
    } else {
        MUD
    }
}

/// True where the site's ground has no water to puddle in.
pub fn is_arid(climate: Climate) -> bool {
    matches!(
        climate,
        Climate::HotDesert | Climate::HotSteppe | Climate::ColdDesert | Climate::ColdSteppe
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prop {
    Heap,
    Stockpile,
    Timber,
    Foundation,
    Cabin,
    Container,
    Scaffold,
    Empty,
}

const PROPS: &[(Prop, u64)] = &[
    (Prop::Heap, 22),
    (Prop::Stockpile, 18),
    (Prop::Timber, 11),
    (Prop::Foundation, 14),
    (Prop::Cabin, 9),
    (Prop::Container, 9),
    (Prop::Scaffold, 12),
    (Prop::Empty, 9),
];

fn pick_prop(roll: u64) -> Prop {
    let total: u64 = PROPS.iter().map(|&(_, w)| w).sum();
    let mut r = roll % total;
    for &(prop, w) in PROPS {
        if r < w {
            return prop;
        }
        r -= w;
    }
    Prop::Empty
}

/// Independent rolls for one plot.
struct Rolls(u64);

impl Rolls {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = coord_hash((self.0 >> 32) as i32, self.0 as i32 ^ 0x51_7E5D);
        self.0 % n.max(1)
    }
}

/// What the site may build on, shared by every prop.
struct Site<'a> {
    cells: FnvHashSet<(i32, i32)>,
    footprints: &'a BuildingFootprintBitmap,
}

impl Site<'_> {
    /// A cell of site ground with nothing built on it. Water and paving the site
    /// kept are not site ground.
    fn open(&self, editor: &WorldEditor, x: i32, z: i32) -> bool {
        self.cells.contains(&(x, z))
            && !self.footprints.contains(x, z)
            && !editor.surface_is_sealed(x, z)
            && !editor.is_lc_water(x, z)
            && editor.check_for_block(x, 0, z, Some(SITE_GROUND))
            && !editor.block_exists_absolute(x, editor.get_absolute_y(x, 1, z), z)
    }

    /// Levels the ground under a rigid prop and returns the first block above it, or
    /// None if any cell is taken or the ground steps by more than one block. The prop
    /// stands at the highest cell's level, with the lower cells filled up to it.
    fn level_pad(&self, editor: &mut WorldEditor, cells: &[(i32, i32)]) -> Option<i32> {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for &(x, z) in cells {
            if !self.open(editor, x, z) {
                return None;
            }
            let y = editor.get_absolute_y(x, 1, z);
            lo = lo.min(y);
            hi = hi.max(y);
        }
        if hi - lo > 1 {
            return None;
        }
        for &(x, z) in cells {
            for y in editor.get_absolute_y(x, 1, z)..hi {
                editor.set_block_absolute(COARSE_DIRT, x, y, z, None, None);
            }
        }
        Some(hi)
    }
}

/// Cells of a `along` by `across` rectangle from (x, z), turned a quarter if `turned`.
/// Returned as (x, z, u, v) with u along and v across.
fn rect(x: i32, z: i32, along: i32, across: i32, turned: bool) -> Vec<(i32, i32, i32, i32)> {
    let mut out = Vec::with_capacity((along * across) as usize);
    for u in 0..along {
        for v in 0..across {
            let (dx, dz) = if turned { (v, u) } else { (u, v) };
            out.push((x + dx, z + dz, u, v));
        }
    }
    out
}

fn xz(cells: &[(i32, i32, i32, i32)]) -> Vec<(i32, i32)> {
    cells.iter().map(|&(x, z, _, _)| (x, z)).collect()
}

/// Lays out the site: a fence along the outline, then one prop per plot. Runs after the
/// ground is painted and after the crane and excavators, which props then keep clear of.
pub fn furnish(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    floor_area: &[(i32, i32)],
    footprints: &BuildingFootprintBitmap,
) {
    if floor_area.is_empty() {
        return;
    }
    let site = Site {
        cells: floor_area.iter().copied().collect(),
        footprints,
    };
    place_fence(editor, element, &site);

    let (mut min_x, mut min_z, mut max_x, mut max_z) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(x, z) in floor_area {
        min_x = min_x.min(x);
        min_z = min_z.min(z);
        max_x = max_x.max(x);
        max_z = max_z.max(z);
    }
    let salt = (element.id ^ (element.id >> 32)) as i32;
    for gx in min_x.div_euclid(PLOT)..=max_x.div_euclid(PLOT) {
        for gz in min_z.div_euclid(PLOT)..=max_z.div_euclid(PLOT) {
            let mut rolls = Rolls(coord_hash(gx ^ salt, gz.wrapping_add(salt)));
            let prop = pick_prop(rolls.next(u64::MAX));
            place_prop(
                editor,
                &site,
                prop,
                gx * PLOT + 1,
                gz * PLOT + 1,
                &mut rolls,
            );
        }
    }
}

/// Mesh fence panels two blocks high along the outline, with gaps where a road or
/// path crosses into the site, at water and at buildings.
fn place_fence(editor: &mut WorldEditor, element: &ProcessedWay, site: &Site) {
    let mut points: Vec<(i32, i32)> = Vec::new();
    for pair in element.nodes.windows(2) {
        for (x, _, z) in bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z) {
            points.push((x, z));
        }
    }
    for (x, z) in four_connected_line(&points) {
        if site.footprints.contains(x, z)
            || editor.surface_is_sealed(x, z)
            || editor.is_lc_water(x, z)
            || editor.check_for_block(x, 0, z, Some(&[WATER]))
        {
            continue;
        }
        let base = editor.get_absolute_y(x, 1, z);
        if editor.block_exists_absolute(x, base, z) {
            continue;
        }
        for y in base..base + 2 {
            place_connected(editor, IRON_BARS, x, y, z);
        }
    }
}

/// Places `prop` somewhere inside the plot whose inner corner is (px, pz). Only the
/// tile owning the prop's corner draws it.
fn place_prop(
    editor: &mut WorldEditor,
    site: &Site,
    prop: Prop,
    px: i32,
    pz: i32,
    rolls: &mut Rolls,
) {
    let turned = rolls.next(2) == 1;
    let (along, across) = match prop {
        Prop::Heap => {
            let r = 2 + rolls.next(2) as i32;
            (2 * r + 1, 2 * r + 1)
        }
        Prop::Stockpile => (2 + rolls.next(3) as i32, 2),
        Prop::Timber => (4 + rolls.next(2) as i32, 2 + rolls.next(2) as i32),
        Prop::Foundation => (5 + rolls.next(5) as i32, 4 + rolls.next(4) as i32),
        Prop::Cabin | Prop::Container => (6, 3),
        Prop::Scaffold => (3 + rolls.next(6) as i32, 1 + rolls.next(3) as i32),
        Prop::Empty => return,
    };
    debug_assert!(along <= MAX_EXTENT && across <= MAX_EXTENT);
    let (w, d) = if turned {
        (across, along)
    } else {
        (along, across)
    };
    let x = px + rolls.next((MAX_EXTENT - w + 1) as u64) as i32;
    let z = pz + rolls.next((MAX_EXTENT - d + 1) as u64) as i32;
    if !editor.owns(x, z) {
        return;
    }
    let cells = rect(x, z, along, across, turned);
    match prop {
        Prop::Heap => place_heap(editor, site, x, z, (along - 1) / 2, rolls),
        Prop::Stockpile => place_stockpile(editor, site, &cells, rolls),
        Prop::Timber => place_timber(editor, site, &cells, turned, rolls),
        Prop::Foundation => place_foundation(editor, site, &cells, along, across, rolls),
        Prop::Cabin => place_box(editor, site, &cells, along, across, rolls, true),
        Prop::Container => place_box(editor, site, &cells, along, across, rolls, false),
        Prop::Scaffold => place_scaffold(editor, site, &cells, along, rolls),
        Prop::Empty => {}
    }
}

/// A cone of sand, gravel or spoil around the plot's centre, following the ground.
fn place_heap(editor: &mut WorldEditor, site: &Site, x: i32, z: i32, r: i32, rolls: &mut Rolls) {
    let (cx, cz) = (x + r, z + r);
    let kind = rolls.next(3);
    for dx in -r..=r {
        for dz in -r..=r {
            let d = ((dx * dx + dz * dz) as f64).sqrt();
            let h = ((r as f64 - d + 0.7).floor() as i32).clamp(0, r);
            if h == 0 || !site.open(editor, cx + dx, cz + dz) {
                continue;
            }
            for y in 1..=h {
                let block = match kind {
                    0 => SAND,
                    1 => GRAVEL,
                    _ if coord_hash(cx + dx, (cz + dz) ^ y).is_multiple_of(3) => COARSE_DIRT,
                    _ => DIRT,
                };
                editor.set_block(block, cx + dx, y, cz + dz, None, None);
            }
        }
    }
}

/// Bricks, blocks or formwork boards stacked one or two high.
fn place_stockpile(
    editor: &mut WorldEditor,
    site: &Site,
    cells: &[(i32, i32, i32, i32)],
    rolls: &mut Rolls,
) {
    let Some(base) = site.level_pad(editor, &xz(cells)) else {
        return;
    };
    let material = [
        BRICK,
        STONE_BRICKS,
        LIGHT_GRAY_CONCRETE,
        SPRUCE_PLANKS,
        SMOOTH_STONE,
    ][rolls.next(5) as usize];
    for &(x, z, _, _) in cells {
        editor.set_block_absolute(material, x, base, z, None, None);
        if rolls.next(4) != 0 {
            editor.set_block_absolute(material, x, base + 1, z, None, None);
        }
    }
}

/// Logs laid lengthwise, one or two layers.
fn place_timber(
    editor: &mut WorldEditor,
    site: &Site,
    cells: &[(i32, i32, i32, i32)],
    turned: bool,
    rolls: &mut Rolls,
) {
    let Some(base) = site.level_pad(editor, &xz(cells)) else {
        return;
    };
    let wood = if rolls.next(2) == 0 {
        SPRUCE_LOG
    } else {
        OAK_LOG
    };
    let log = cached_prop_block(wood, &[("axis", if turned { "z" } else { "x" })]);
    let two_layers = rolls.next(2) == 0;
    let across = cells.iter().map(|&(_, _, _, v)| v).max().unwrap_or(0);
    for &(x, z, _, v) in cells {
        editor.set_block_with_properties_absolute(log.clone(), x, base, z, None, None);
        // The top layer leaves the last row bare, so the stack steps down.
        if two_layers && v < across {
            editor.set_block_with_properties_absolute(log.clone(), x, base + 1, z, None, None);
        }
    }
}

/// A poured slab flush with the ground, edged with formwork boards, with starter bars
/// standing up from it.
fn place_foundation(
    editor: &mut WorldEditor,
    site: &Site,
    cells: &[(i32, i32, i32, i32)],
    along: i32,
    across: i32,
    rolls: &mut Rolls,
) {
    let Some(base) = site.level_pad(editor, &xz(cells)) else {
        return;
    };
    let tall_bars = rolls.next(2) == 0;
    for &(x, z, u, v) in cells {
        let edge = u == 0 || v == 0 || u == along - 1 || v == across - 1;
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base - 1, z, Some(SITE_GROUND), None);
        if edge {
            editor.set_block_absolute(SPRUCE_SLAB, x, base, z, None, None);
        } else if u % 2 == 1 && v % 2 == 1 {
            editor.set_block_absolute(IRON_BARS, x, base, z, None, None);
            if tall_bars {
                editor.set_block_absolute(IRON_BARS, x, base + 1, z, None, None);
            }
        }
    }
}

/// A site cabin (white, windowed, with a doorway) or a shipping container (one plain
/// colour), hollow and three blocks high.
#[allow(clippy::too_many_arguments)]
fn place_box(
    editor: &mut WorldEditor,
    site: &Site,
    cells: &[(i32, i32, i32, i32)],
    along: i32,
    across: i32,
    rolls: &mut Rolls,
    cabin: bool,
) {
    let Some(base) = site.level_pad(editor, &xz(cells)) else {
        return;
    };
    let wall = if cabin {
        if rolls.next(4) == 0 {
            LIGHT_GRAY_CONCRETE
        } else {
            WHITE_CONCRETE
        }
    } else {
        [
            BLUE_CONCRETE,
            ORANGE_CONCRETE,
            RED_CONCRETE,
            GREEN_CONCRETE,
            CYAN_CONCRETE,
            GRAY_CONCRETE,
        ][rolls.next(6) as usize]
    };
    let door_end = if rolls.next(2) == 0 { 0 } else { along - 1 };
    for &(x, z, u, v) in cells {
        let end = u == 0 || u == along - 1;
        let side = v == 0 || v == across - 1;
        for dy in 0..2 {
            if !(end || side) {
                continue;
            }
            let block = if cabin && end && u == door_end && v == across / 2 {
                continue;
            } else if cabin && side && !end && dy == 1 && u % 2 == 1 {
                GLASS
            } else {
                wall
            };
            editor.set_block_absolute(block, x, base + dy, z, None, None);
        }
        editor.set_block_absolute(wall, x, base + 2, z, None, None);
    }
}

/// A run of scaffolding, one to three columns deep, stepping up and down along its
/// length, with the odd column left out.
fn place_scaffold(
    editor: &mut WorldEditor,
    site: &Site,
    cells: &[(i32, i32, i32, i32)],
    along: i32,
    rolls: &mut Rolls,
) {
    let Some(base) = site.level_pad(editor, &xz(cells)) else {
        return;
    };
    // Height per slice along the run, a random walk between 2 and 7.
    let mut heights = Vec::with_capacity(along as usize);
    let mut h = 3 + rolls.next(4) as i32;
    for _ in 0..along {
        heights.push(h);
        h = (h + rolls.next(3) as i32 - 1).clamp(2, 7);
    }
    let scaffold = cached_prop_block(
        SCAFFOLDING,
        &[
            ("distance", "0"),
            ("bottom", "false"),
            ("waterlogged", "false"),
        ],
    );
    for &(x, z, u, v) in cells {
        let end = u == 0 || u == along - 1;
        if !end && rolls.next(8) == 0 {
            continue;
        }
        // Back rows sometimes stop a level short.
        let height = heights[u as usize] - (v > 0 && rolls.next(3) == 0) as i32;
        for y in base..base + height {
            editor.set_block_with_properties_absolute(scaffold.clone(), x, y, z, None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{rect_way, test_editor};
    use crate::floodfill_cache::SealedSurfaceBitmap;
    use std::sync::Arc;

    #[test]
    fn ground_comes_in_patches_of_several_kinds() {
        let mut counts = std::collections::HashMap::new();
        let mut changes = 0;
        for x in 0..200 {
            for z in 0..200 {
                let b = ground_block(x, z, false);
                *counts.entry(b).or_insert(0) += 1;
                if x > 0 && ground_block(x - 1, z, false) != b {
                    changes += 1;
                }
            }
        }
        for kind in SITE_GROUND {
            let share = counts.get(kind).copied().unwrap_or(0) as f64 / 40_000.0;
            assert!(share > 0.03, "{} covers {share:.3}", kind.name());
        }
        // Patches, not noise: most neighbours match.
        assert!(changes < 40_000 / 4, "{changes} changes along x");
        assert!(
            (0..200).all(|x| ground_block(x, 7, true) != MUD),
            "no mud when arid"
        );
    }

    fn furnished(sealed_row: Option<i32>) -> (WorldEditor<'static>, ProcessedWay) {
        let xzbbox = Box::leak(Box::new(
            XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap(),
        ));
        let mut editor = test_editor(xzbbox);
        if let Some(z) = sealed_row {
            let mut mask = SealedSurfaceBitmap::new(xzbbox);
            for x in 0..120 {
                mask.set(x, z);
            }
            editor.set_sealed_surface(Arc::new(mask));
        }
        let way = rect_way(7, 10, 10, 100, 100, &[("landuse", "construction")]);
        let area: Vec<(i32, i32)> = (10..=100)
            .flat_map(|x| (10..=100).map(move |z| (x, z)))
            .collect();
        for &(x, z) in &area {
            editor.set_block(ground_block(x, z, false), x, 0, z, None, None);
        }
        let footprints = BuildingFootprintBitmap::new_empty();
        furnish(&mut editor, &way, &area, &footprints);
        (editor, way)
    }

    #[test]
    fn a_site_is_fenced_with_a_gate_where_a_road_enters() {
        let (editor, _) = furnished(Some(50));
        let bars_at = |x: i32, z: i32| {
            editor.check_for_block(x, 1, z, Some(&[IRON_BARS]))
                && editor.check_for_block(x, 2, z, Some(&[IRON_BARS]))
        };
        assert!(bars_at(10, 30), "west side fenced");
        assert!(bars_at(60, 100), "south side fenced");
        assert!(!bars_at(10, 50), "a gap where the road crosses");
        assert!(!editor.check_for_block(10, 1, 50, Some(&[IRON_BARS])));
    }

    #[test]
    fn a_site_gets_several_kinds_of_prop_and_none_on_the_road() {
        let (editor, _) = furnished(Some(50));
        let looks_for = [
            SAND,
            GRAVEL,
            DIRT,
            BRICK,
            STONE_BRICKS,
            LIGHT_GRAY_CONCRETE,
            SPRUCE_PLANKS,
            SMOOTH_STONE,
            SPRUCE_LOG,
            OAK_LOG,
            SPRUCE_SLAB,
            WHITE_CONCRETE,
            SCAFFOLDING,
            BLUE_CONCRETE,
            ORANGE_CONCRETE,
            RED_CONCRETE,
            GREEN_CONCRETE,
            CYAN_CONCRETE,
            GRAY_CONCRETE,
        ];
        let mut seen = std::collections::HashSet::new();
        for x in 11..100 {
            for z in 11..100 {
                let y = editor.get_absolute_y(x, 1, z);
                if let Some(b) = editor.get_block_absolute(x, y, z) {
                    if z == 50 {
                        panic!("{} on the road at x={x}", b.name());
                    }
                    if looks_for.contains(&b) {
                        seen.insert(b);
                    }
                }
            }
        }
        assert!(seen.len() >= 5, "only {} kinds of prop", seen.len());
    }

    #[test]
    fn scaffolding_varies_in_size_and_height() {
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let footprints = BuildingFootprintBitmap::new_empty();
        let site = Site {
            cells: (0..400)
                .flat_map(|x| (0..40).map(move |z| (x, z)))
                .collect(),
            footprints: &footprints,
        };
        for &(x, z) in &site.cells {
            editor.set_block(COARSE_DIRT, x, 0, z, None, None);
        }
        let mut shapes = std::collections::HashSet::new();
        for i in 0..20 {
            let mut rolls = Rolls(coord_hash(i, 99));
            let (along, across) = (3 + rolls.next(6) as i32, 1 + rolls.next(3) as i32);
            let x0 = 5 + i * 18;
            let cells = rect(x0, 5, along, across, false);
            place_scaffold(&mut editor, &site, &cells, along, &mut rolls);
            let mut columns = 0;
            let mut tops = std::collections::HashSet::new();
            for x in x0..x0 + along {
                for z in 5..5 + across {
                    let h = (1..=8)
                        .take_while(|&y| editor.check_for_block(x, y, z, Some(&[SCAFFOLDING])))
                        .count();
                    if h > 0 {
                        columns += 1;
                        tops.insert(h);
                    }
                }
            }
            assert!(columns > 0, "scaffold {i} stood");
            shapes.insert((columns, tops.len() > 1));
        }
        assert!(shapes.len() >= 4, "scaffolds look alike: {shapes:?}");
        assert!(
            shapes.iter().any(|&(_, stepped)| stepped),
            "none step in height"
        );
    }

    #[test]
    fn a_prop_over_a_step_stands_level_on_the_higher_ground() {
        let xzbbox = XZBBox::rect_from_xz_lengths(30.0, 30.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let footprints = BuildingFootprintBitmap::new_empty();
        let site = Site {
            cells: (0..30).flat_map(|x| (0..30).map(move |z| (x, z))).collect(),
            footprints: &footprints,
        };
        // Ground one block higher from x = 6 on.
        for &(x, z) in &site.cells {
            if x >= 6 {
                editor.register_road_surface_y(x, z, 1);
            }
            editor.set_block(COARSE_DIRT, x, 0, z, None, None);
        }
        let cells = rect(3, 5, 6, 3, false);
        place_box(&mut editor, &site, &cells, 6, 3, &mut Rolls(7), false);
        for &(x, z, u, v) in &cells {
            let wall = u == 0 || u == 5 || v == 0 || v == 2;
            assert_eq!(
                editor.block_exists_absolute(x, 2, z),
                wall,
                "walls start at the higher level at ({x}, {z})"
            );
            assert!(
                editor.block_exists_absolute(x, 1, z),
                "no gap under the box at ({x}, {z})"
            );
        }
    }

    #[test]
    fn props_and_fence_keep_off_water_the_site_kept() {
        let xzbbox = Box::leak(Box::new(
            XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap(),
        ));
        let mut editor = test_editor(xzbbox);
        let way = rect_way(7, 10, 10, 100, 100, &[("landuse", "construction")]);
        let area: Vec<(i32, i32)> = (10..=100)
            .flat_map(|x| (10..=100).map(move |z| (x, z)))
            .collect();
        // A pond across the west half, then site ground on the rest.
        for &(x, z) in &area {
            let block = if x < 55 {
                WATER
            } else {
                ground_block(x, z, false)
            };
            editor.set_block(block, x, 0, z, None, None);
        }
        furnish(
            &mut editor,
            &way,
            &area,
            &BuildingFootprintBitmap::new_empty(),
        );
        for x in 10..55 {
            for z in 10..=100 {
                assert!(
                    !editor.block_exists_absolute(x, 1, z),
                    "something stands on the pond at ({x}, {z})"
                );
            }
        }
        assert!(
            editor.check_for_block(100, 1, 50, Some(&[IRON_BARS])),
            "dry side fenced"
        );
    }
}
