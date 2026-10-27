//! Swimming pools: a level basin lined in tile, rimmed with coping, with lane lines in
//! lap pools.

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::sport_pitches::longest_edge_dir;
use crate::floodfill_cache::{
    edge_in_hole, in_hole, is_oversized_ring, BuildingFootprintBitmap, FloodFillCache, Holes,
    InnerRings, RoadMaskBitmap,
};
use crate::ground_decoration::{LOOSE_PLANTS, STACKED_PLANT_PARTS};
use crate::mapillary::geometry::parse_length_m;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::water_depth::{clear_tree_from, is_tree_part};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub(crate) const COPING: Block = SMOOTH_QUARTZ;
pub(crate) const LINING: Block = LIGHT_BLUE_CONCRETE;
const LANE_LINE: Block = BLUE_CONCRETE;

const DEFAULT_DEPTH_M: f64 = 2.0;
const MAX_DEPTH: i32 = 5;
/// Pools smaller than this, in square metres, are plunge pools and tubs.
const PLUNGE_POOL_M2: f64 = 12.0;

/// Ground the coping may replace, and what marks a building's floor where it is absent.
const NATURAL_GROUND: &[Block] = &[GRASS_BLOCK, DIRT, COARSE_DIRT, PODZOL, MUD, SAND, GRAVEL];

pub fn is_swimming_pool(tags: &HashMap<String, String>) -> bool {
    tags.get("leisure").map(String::as_str) == Some("swimming_pool")
}

pub struct PoolContext<'a> {
    pub flood_fill_cache: &'a FloodFillCache,
    pub building_footprints: &'a BuildingFootprintBitmap,
    pub road_mask: &'a RoadMaskBitmap,
    pub timeout: Option<&'a Duration>,
}

pub fn generate_swimming_pool(editor: &mut WorldEditor, way: &ProcessedWay, ctx: &PoolContext) {
    pool(editor, way, ctx, None);
}

/// Each outer ring is its own pool, less the islands its inner rings cut.
pub fn generate_swimming_pool_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    ctx: &PoolContext,
) {
    let inners = InnerRings::of(rel);
    for member in &rel.members {
        if member.role != ProcessedMemberRole::Outer {
            continue;
        }
        let holes = ctx
            .flood_fill_cache
            .holes_in(&member.way, &inners, ctx.timeout);
        let way = ProcessedWay {
            id: member.way.id,
            nodes: member.way.nodes.clone(),
            tags: rel.tags.clone(),
        };
        pool(editor, &way, ctx, holes.as_ref());
    }
}

fn pool(editor: &mut WorldEditor, way: &ProcessedWay, ctx: &PoolContext, holes: Option<&Holes>) {
    let closed = way.nodes.len() >= 4
        && way.nodes.first().map(|n| (n.x, n.z)) == way.nodes.last().map(|n| (n.x, n.z));
    if !closed {
        return;
    }
    let fill = ctx.flood_fill_cache.get_or_compute(way, ctx.timeout);
    if fill.is_empty() && is_oversized_ring(way) {
        return;
    }

    // The mapped ring is the water's edge, so its own cells hold water too.
    let mut cells: HashSet<(i32, i32)> = fill
        .iter()
        .copied()
        .filter(|&(x, z)| !in_hole(holes, x, z))
        .collect();
    for pair in way.nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            if !edge_in_hole(holes, x, z) {
                cells.insert((x, z));
            }
        }
    }
    // An indoor pool stays under its hall's floor; a canopy has none, so the pool runs on.
    cells.retain(|&(x, z)| !under_a_floor(editor, ctx.building_footprints, x, z));
    let Some(water_y) = cells
        .iter()
        .map(|&(x, z)| editor.get_ground_level(x, z))
        .max()
    else {
        return;
    };

    // The surface sits at the highest ground under the pool, so on a slope the low end
    // stands on a wall instead of the high end being buried.
    let scale = editor.scale();
    let depth = pool_depth(&way.tags, cells.len(), scale);
    let floor_y = water_y - depth;
    let lanes = lane_cells(way, &cells, scale);

    for &(x, z) in &cells {
        let ground_y = editor.get_ground_level(x, z);
        let floor = if lanes.contains(&(x, z)) {
            LANE_LINE
        } else {
            LINING
        };
        place(editor, floor, x, floor_y, z, ground_y);
        for y in floor_y + 1..=water_y {
            place(editor, WATER, x, y, z, ground_y);
        }
        editor.fill_column_absolute(LINING, x, z, ground_y.min(floor_y - 1), floor_y - 1, true);
        clear_above(editor, x, water_y, z);
    }

    let mut rim: Vec<(i32, i32)> = cells
        .iter()
        .flat_map(|&(x, z)| {
            [-1, 0, 1]
                .into_iter()
                .flat_map(move |dx| [-1, 0, 1].into_iter().map(move |dz| (x + dx, z + dz)))
        })
        .filter(|&(x, z)| {
            !cells.contains(&(x, z)) && !under_a_floor(editor, ctx.building_footprints, x, z)
        })
        .collect();
    rim.sort_unstable();
    rim.dedup();

    for (x, z) in rim {
        let ground_y = editor.get_ground_level(x, z);
        let top = water_y.max(ground_y);
        if top > ground_y {
            // A wall standing clear of the ground, coping all the way down, even beside a path.
            editor.fill_column_absolute(COPING, x, z, ground_y + 1, top, true);
        } else if !ctx.road_mask.contains(x, z) {
            editor.set_block_absolute(COPING, x, top, z, Some(NATURAL_GROUND), None);
        }
        editor.fill_column_absolute(
            LINING,
            x,
            z,
            (floor_y - 1).min(ground_y),
            ground_y.min(top - 1),
            true,
        );
    }
}

/// Inside a building that laid a floor at ground level, unlike a roof on posts.
fn under_a_floor(
    editor: &WorldEditor,
    footprints: &BuildingFootprintBitmap,
    x: i32,
    z: i32,
) -> bool {
    footprints.contains(x, z)
        && editor
            .get_block_absolute(x, editor.get_ground_level(x, z), z)
            .is_some_and(|b| !NATURAL_GROUND.contains(&b))
}

/// Writes a pool block. Below ground it replaces whatever is there; above it only air,
/// plants, trees and another pool's blocks, so a deck or bridge over the pool stays.
fn place(editor: &mut WorldEditor, block: Block, x: i32, y: i32, z: i32, ground_y: i32) {
    if y <= ground_y {
        editor.set_block_absolute(block, x, y, z, None, Some(&[]));
        return;
    }
    match editor.get_block_absolute(x, y, z) {
        None => editor.set_block_absolute(block, x, y, z, None, None),
        Some(existing) if is_tree_part(existing) => {
            clear_tree_from(editor, x, y, z);
            editor.set_block_absolute(block, x, y, z, None, None);
        }
        Some(existing)
            if LOOSE_PLANTS.contains(&existing)
                || STACKED_PLANT_PARTS.contains(&existing)
                || [COPING, LINING, LANE_LINE].contains(&existing) =>
        {
            editor.set_block_absolute(block, x, y, z, None, Some(&[]));
        }
        Some(_) => {}
    }
}

/// Water depth in blocks: `depth=*` when tagged, else from the pool's kind and size.
fn pool_depth(tags: &HashMap<String, String>, cells: usize, scale: f64) -> i32 {
    let tagged = tags
        .get("depth")
        .and_then(|d| parse_length_m(d))
        .filter(|d| d.is_finite() && *d > 0.0);
    let metres = tagged.unwrap_or_else(|| match tags.get("swimming_pool").map(String::as_str) {
        Some("wading" | "paddling" | "kiddie" | "toddler" | "children") => 0.5,
        Some("diving") => 4.0,
        _ if (cells as f64) < PLUNGE_POOL_M2 * scale * scale => 1.0,
        _ => DEFAULT_DEPTH_M,
    });
    ((metres * scale).round() as i32).clamp(1, MAX_DEPTH)
}

/// Whether the tags allow lane lines, and the smallest length and width in metres.
fn lap_pool_size(tags: &HashMap<String, String>) -> Option<(f64, f64)> {
    let kind = tags.get("swimming_pool").map(String::as_str);
    if matches!(
        kind,
        Some(
            "wading"
                | "paddling"
                | "kiddie"
                | "toddler"
                | "children"
                | "diving"
                | "plunge"
                | "plunge_pool"
                | "hot_tub"
                | "whirlpool"
                | "spa"
                | "wave"
                | "ornamental"
                | "reflecting"
        )
    ) {
        return None;
    }
    let for_laps = matches!(kind, Some("swimming" | "lap" | "competition"))
        || tags
            .get("sport")
            .is_some_and(|s| s.split(';').any(|s| s.trim() == "swimming"));
    // Untagged, only a public pool's size reads as one built for laps.
    Some(if for_laps { (20.0, 7.0) } else { (25.0, 10.0) })
}

/// Floor cells under the lane lines of a rectangular lap pool, empty for any other.
/// Each line stops short of the walls.
fn lane_cells(way: &ProcessedWay, cells: &HashSet<(i32, i32)>, scale: f64) -> HashSet<(i32, i32)> {
    let mut lines = HashSet::new();
    let Some((min_length, min_width)) = lap_pool_size(&way.tags) else {
        return lines;
    };
    let Some((mut ux, mut uz)) = longest_edge_dir(way) else {
        return lines;
    };
    let (mut vx, mut vz) = (-uz, ux);
    let n = cells.len() as f64;
    let cx = cells.iter().map(|&(x, _)| x as f64).sum::<f64>() / n;
    let cz = cells.iter().map(|&(_, z)| z as f64).sum::<f64>() / n;

    let extent = |ux: f64, uz: f64| {
        cells
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &(x, z)| {
                let u = (x as f64 - cx) * ux + (z as f64 - cz) * uz;
                (lo.min(u), hi.max(u))
            })
    };
    let (mut u_min, mut u_max) = extent(ux, uz);
    let (mut v_min, mut v_max) = extent(vx, vz);
    if v_max - v_min > u_max - u_min {
        std::mem::swap(&mut ux, &mut vx);
        std::mem::swap(&mut uz, &mut vz);
        std::mem::swap(&mut u_min, &mut v_min);
        std::mem::swap(&mut u_max, &mut v_max);
    }
    let (length, width) = (u_max - u_min + 1.0, v_max - v_min + 1.0);
    if length < min_length * scale || width < min_width * scale || n < 0.85 * length * width {
        return lines;
    }

    // Lines are a block wide, so below scale 1 the lanes stay 2.5 blocks and fewer fit.
    let lane = 2.5 * scale.max(1.0);
    let count = (width / lane).floor();
    if count < 2.0 {
        return lines;
    }
    let first = (v_min + v_max) / 2.0 - count * lane / 2.0 + lane / 2.0;
    let u_mid = (u_min + u_max) / 2.0;
    let line_end = length / 2.0 - (2.0 * scale).max(1.5);
    // Cross bars at the line ends only where the lanes keep them apart.
    let bar = lane >= 4.0;
    // Half-open, so a line falling between two rows takes exactly one of them.
    let on = |d: f64, half: f64| d > -half && d <= half;
    for &(x, z) in cells {
        let (dx, dz) = (x as f64 - cx, z as f64 - cz);
        let u = dx * ux + dz * uz - u_mid;
        let v = dx * vx + dz * vz;
        let marked = (0..count as i32).any(|k| {
            let dv = v - (first + f64::from(k) * lane);
            (on(dv, 0.5) && u.abs() <= line_end)
                || (bar && on(dv, 1.5) && on(u.abs() - line_end, 0.5))
        });
        if marked {
            lines.insert((x, z));
        }
    }
    lines
}

/// Strips plants and trees that an earlier area left standing where the water now is.
fn clear_above(editor: &mut WorldEditor, x: i32, water_y: i32, z: i32) {
    if editor
        .get_block_absolute(x, water_y + 1, z)
        .is_some_and(is_tree_part)
    {
        clear_tree_from(editor, x, water_y + 1, z);
    }
    editor.set_block_absolute(AIR, x, water_y + 1, z, Some(LOOSE_PLANTS), None);
    editor.set_block_absolute(AIR, x, water_y + 2, z, Some(STACKED_PLANT_PARTS), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::ground::Ground;
    use crate::osm_parser::{ProcessedMember, ProcessedNode};
    use std::path::PathBuf;
    use std::sync::Arc;

    const SIDE: usize = 64;

    fn bbox() -> XZBBox {
        XZBBox::rect_from_min_max(0, 0, SIDE as i32 - 1, SIDE as i32 - 1).unwrap()
    }

    fn editor(bbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), bbox, llbbox)
    }

    /// Terrain rising `per_block` blocks per step along X from `base`.
    fn editor_over_slope(bbox: &XZBBox, base: f64, per_block: f64) -> WorldEditor<'_> {
        let heights: Vec<Vec<f32>> = (0..SIDE)
            .map(|_| {
                (0..SIDE)
                    .map(|x| (base + per_block * x as f64) as f32)
                    .collect()
            })
            .collect();
        let mut editor = editor(bbox);
        editor.set_ground(Arc::new(Ground::new_elevation_test(heights, SIDE, SIDE)));
        editor
    }

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ring(id: u64, x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<ProcessedNode> {
        [(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: id * 10 + (i % 4) as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect()
    }

    fn pool_way(
        id: u64,
        x0: i32,
        z0: i32,
        x1: i32,
        z1: i32,
        extra: &[(&str, &str)],
    ) -> ProcessedWay {
        let mut all = tags(extra);
        all.insert("leisure".to_string(), "swimming_pool".to_string());
        ProcessedWay {
            id,
            nodes: ring(id, x0, z0, x1, z1),
            tags: all,
        }
    }

    fn pool(x0: i32, z0: i32, x1: i32, z1: i32, extra: &[(&str, &str)]) -> ProcessedWay {
        pool_way(1, x0, z0, x1, z1, extra)
    }

    struct Masks {
        footprints: BuildingFootprintBitmap,
        roads: RoadMaskBitmap,
        cache: FloodFillCache,
    }

    impl Masks {
        fn new(bbox: &XZBBox) -> Self {
            Masks {
                footprints: BuildingFootprintBitmap::new(bbox),
                roads: RoadMaskBitmap::new(bbox),
                cache: FloodFillCache::new(),
            }
        }

        fn ctx(&self) -> PoolContext<'_> {
            PoolContext {
                flood_fill_cache: &self.cache,
                building_footprints: &self.footprints,
                road_mask: &self.roads,
                timeout: None,
            }
        }
    }

    fn generate(editor: &mut WorldEditor, way: &ProcessedWay, masks: &Masks) {
        generate_swimming_pool(editor, way, &masks.ctx());
    }

    fn is(editor: &WorldEditor, x: i32, y: i32, z: i32, block: Block) -> bool {
        editor.check_for_block_absolute(x, y, z, Some(&[block]), None)
    }

    /// Every water block has water or something solid on each side and below.
    fn assert_contained(editor: &WorldEditor, y_range: std::ops::RangeInclusive<i32>) {
        for x in 0..SIDE as i32 {
            for z in 0..SIDE as i32 {
                for y in y_range.clone() {
                    if !is(editor, x, y, z, WATER) {
                        continue;
                    }
                    for (nx, ny, nz) in [
                        (x + 1, y, z),
                        (x - 1, y, z),
                        (x, y, z + 1),
                        (x, y, z - 1),
                        (x, y - 1, z),
                    ] {
                        assert!(
                            editor.block_exists_absolute(nx, ny, nz),
                            "water at ({x}, {y}, {z}) leaks into ({nx}, {ny}, {nz})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_pool_is_a_lined_basin_rimmed_with_coping() {
        let bbox = bbox();
        let mut editor = editor(&bbox);
        let masks = Masks::new(&bbox);
        generate(&mut editor, &pool(10, 10, 19, 15, &[]), &masks);

        for (x, z) in [(10, 10), (14, 12), (19, 15)] {
            assert!(is(&editor, x, 0, z, WATER), "surface at ({x}, {z})");
            assert!(is(&editor, x, -1, z, WATER), "second block at ({x}, {z})");
            assert!(is(&editor, x, -2, z, LINING), "floor at ({x}, {z})");
        }
        for (x, z) in [(9, 9), (9, 12), (20, 16), (14, 16)] {
            assert!(is(&editor, x, 0, z, COPING), "coping at ({x}, {z})");
            assert!(is(&editor, x, -1, z, LINING), "wall at ({x}, {z})");
            assert!(is(&editor, x, -2, z, LINING), "wall at ({x}, {z})");
        }
        assert!(
            !editor.block_exists_absolute(8, 0, 12),
            "coping is one block wide"
        );
        assert_contained(&editor, -4..=2);
    }

    #[test]
    fn a_pool_on_a_slope_stays_level_and_walls_its_low_end() {
        let bbox = bbox();
        let mut editor = editor_over_slope(&bbox, 4.0, 0.25);
        let low = editor.get_ground_level(10, 12);
        let high = editor.get_ground_level(21, 12);
        assert!(high >= low + 2, "fixture no longer slopes: {low}..{high}");
        let masks = Masks::new(&bbox);
        generate(&mut editor, &pool(10, 10, 21, 15, &[]), &masks);

        for x in 10..=21 {
            assert!(
                is(&editor, x, high, 12, WATER),
                "surface not level at x={x}"
            );
            assert!(!editor.block_exists_absolute(x, high + 1, 12));
        }
        let rim_ground = editor.get_ground_level(9, 12);
        assert!(is(&editor, 9, high, 12, COPING), "low end coping");
        for y in rim_ground + 1..high {
            assert!(is(&editor, 9, y, 12, COPING), "low end wall open at y={y}");
        }
        for y in low..high - 2 {
            assert!(
                is(&editor, 10, y, 12, LINING),
                "basin hollow under the low end at y={y}"
            );
        }
        assert_contained(&editor, 0..=20);
    }

    #[test]
    fn a_deck_over_the_low_end_stays_and_plants_go() {
        let bbox = bbox();
        let mut editor = editor_over_slope(&bbox, 4.0, 0.25);
        let low = editor.get_ground_level(10, 12);
        editor.set_block_absolute(OAK_PLANKS, 10, low + 1, 12, None, None);
        editor.set_block_absolute(GRASS, 11, low + 1, 13, None, None);
        let masks = Masks::new(&bbox);
        generate(&mut editor, &pool(10, 10, 21, 15, &[]), &masks);
        assert!(is(&editor, 10, low + 1, 12, OAK_PLANKS), "deck replaced");
        assert!(
            is(&editor, 11, low + 1, 13, WATER),
            "plant left in the pool"
        );
    }

    #[test]
    fn a_lap_pool_gets_lane_lines_and_other_pools_do_not() {
        let bbox = bbox();
        let masks = Masks::new(&bbox);

        let mut lap = editor(&bbox);
        generate(&mut lap, &pool(5, 5, 29, 16, &[]), &masks);
        // 25 by 12 holds four lanes, so four lines run along the length.
        let lines: Vec<i32> = (5..=16)
            .filter(|&z| is(&lap, 17, -2, z, LANE_LINE))
            .collect();
        assert_eq!(lines.len(), 4, "lane lines at {lines:?}");
        assert!(
            !is(&lap, 5, -2, lines[0], LANE_LINE),
            "lines stop short of the wall"
        );

        for (extra, why) in [
            (&[][..], "garden pool"),
            (&[("swimming_pool", "diving")][..], "diving pool"),
        ] {
            let (x1, z1) = if extra.is_empty() { (14, 9) } else { (29, 16) };
            let mut other = editor(&bbox);
            generate(&mut other, &pool(5, 5, x1, z1, extra), &masks);
            for x in 5..=x1 {
                for z in 5..=z1 {
                    assert!(
                        !is(&other, x, -2, z, LANE_LINE),
                        "{why}: lane at ({x}, {z})"
                    );
                }
            }
        }
    }

    #[test]
    fn lane_lines_stay_apart_at_any_scale() {
        let cells = |x1: i32, z1: i32| -> HashSet<(i32, i32)> {
            (0..=x1)
                .flat_map(|x| (0..=z1).map(move |z| (x, z)))
                .collect()
        };
        let lap = |x1, z1| pool_way(1, 0, 0, x1, z1, &[("sport", "swimming")]);

        // A 50 by 25 m pool at half scale: lines a block wide, a lane of 2.5 blocks apart.
        let half = lane_cells(&lap(24, 12), &cells(24, 12), 0.5);
        assert!(!half.is_empty());
        assert!(
            half.len() * 2 < cells(24, 12).len(),
            "{} of {} floor cells are lines",
            half.len(),
            cells(24, 12).len()
        );
        // Tiny at a small scale: the floor is not painted solid.
        assert!(lane_cells(&lap(7, 3), &cells(7, 3), 0.3).len() * 3 < 32);

        // At double scale the lanes are wide enough for cross bars.
        let double = lane_cells(&lap(49, 23), &cells(49, 23), 2.0);
        let line_rows: HashSet<i32> = double.iter().filter(|c| c.0 == 25).map(|c| c.1).collect();
        let bar_rows: HashSet<i32> = double.iter().filter(|c| c.0 == 3).map(|c| c.1).collect();
        assert!(bar_rows.len() > line_rows.len(), "no cross bars");
    }

    #[test]
    fn an_indoor_pool_leaves_its_hall_alone_but_runs_on_under_a_canopy() {
        let bbox = bbox();
        let mut masks = Masks::new(&bbox);
        for x in 8..=21 {
            for z in 8..=17 {
                masks.footprints.set(x, z);
            }
        }

        let mut hall = editor(&bbox);
        for x in 8..=21 {
            for z in 8..=17 {
                hall.set_block_absolute(STONE_BRICKS, x, 0, z, None, None);
            }
        }
        generate(&mut hall, &pool(10, 10, 19, 15, &[]), &masks);
        for x in 0..SIDE as i32 {
            for z in 0..SIDE as i32 {
                assert!(!is(&hall, x, 0, z, WATER), "water at ({x}, {z})");
                assert!(!is(&hall, x, 0, z, COPING), "coping at ({x}, {z})");
            }
        }

        let mut canopy = editor(&bbox);
        generate(&mut canopy, &pool(10, 10, 19, 15, &[]), &masks);
        assert!(is(&canopy, 14, 0, 12, WATER));
        assert!(is(&canopy, 9, 0, 12, COPING));
    }

    #[test]
    fn coping_does_not_pave_over_a_path() {
        let bbox = bbox();
        let mut editor = editor(&bbox);
        let mut masks = Masks::new(&bbox);
        // A compacted path renders as gravel, which reads as natural ground by block alone.
        for (x, z) in [(9, 12), (9, 13)] {
            editor.set_block_absolute(GRAVEL, x, 0, z, None, None);
        }
        masks.roads.set(9, 12);
        generate(&mut editor, &pool(10, 10, 19, 15, &[]), &masks);
        assert!(is(&editor, 9, 0, 12, GRAVEL), "path paved over");
        assert!(is(&editor, 9, 0, 13, COPING));
        assert!(
            is(&editor, 9, -1, 12, LINING),
            "the wall still runs under the path"
        );
    }

    #[test]
    fn neighbouring_pools_at_two_levels_keep_a_wall_between_them() {
        let bbox = bbox();
        for order in [[0, 1], [1, 0]] {
            let mut editor = editor_over_slope(&bbox, 4.0, 0.25);
            let masks = Masks::new(&bbox);
            let pools = [
                pool_way(1, 6, 10, 15, 15, &[]),
                pool_way(2, 16, 10, 25, 15, &[]),
            ];
            for i in order {
                generate(&mut editor, &pools[i], &masks);
            }
            let (low, high) = (
                editor.get_ground_level(15, 12),
                editor.get_ground_level(25, 12),
            );
            assert!(high > low, "fixture no longer slopes");
            assert_contained(&editor, 0..=20);
        }
    }

    #[test]
    fn a_multipolygon_pool_keeps_its_island() {
        let bbox = bbox();
        let mut editor = editor(&bbox);
        let masks = Masks::new(&bbox);
        let member = |id, role, nodes| ProcessedMember {
            role,
            way: Arc::new(ProcessedWay {
                id,
                nodes,
                tags: HashMap::new(),
            }),
        };
        let rel = ProcessedRelation {
            id: 9,
            tags: tags(&[("leisure", "swimming_pool"), ("type", "multipolygon")]),
            members: vec![
                member(1, ProcessedMemberRole::Outer, ring(1, 10, 10, 30, 30)),
                member(2, ProcessedMemberRole::Inner, ring(2, 18, 18, 22, 22)),
            ],
        };
        generate_swimming_pool_from_relation(&mut editor, &rel, &masks.ctx());
        assert!(is(&editor, 12, 0, 12, WATER));
        assert!(!is(&editor, 20, 0, 20, WATER), "island flooded");
    }

    #[test]
    fn depth_follows_the_tags_the_size_and_the_scale() {
        assert_eq!(pool_depth(&tags(&[]), 60, 1.0), 2);
        assert_eq!(pool_depth(&tags(&[]), 6, 1.0), 1);
        assert_eq!(pool_depth(&tags(&[("depth", "3.5 m")]), 60, 1.0), 4);
        assert_eq!(pool_depth(&tags(&[("depth", "1,5")]), 60, 1.0), 2);
        assert_eq!(pool_depth(&tags(&[("depth", "40")]), 60, 1.0), MAX_DEPTH);
        assert_eq!(pool_depth(&tags(&[("depth", "350 mm")]), 60, 1.0), 2);
        assert_eq!(pool_depth(&tags(&[("depth", "deep")]), 60, 1.0), 2);
        assert_eq!(
            pool_depth(&tags(&[("swimming_pool", "wading")]), 60, 1.0),
            1
        );
        assert_eq!(
            pool_depth(&tags(&[("swimming_pool", "diving")]), 60, 1.0),
            4
        );
        // At double scale a 10 m² tub covers 40 cells and still gets a metre.
        assert_eq!(pool_depth(&tags(&[]), 40, 2.0), 2);
        assert_eq!(pool_depth(&tags(&[]), 200, 2.0), 4);

        let bbox = bbox();
        let mut editor = editor(&bbox);
        let masks = Masks::new(&bbox);
        generate(
            &mut editor,
            &pool(10, 10, 19, 15, &[("depth", "4")]),
            &masks,
        );
        assert!(is(&editor, 14, -3, 12, WATER));
        assert!(is(&editor, 14, -4, 12, LINING));
        assert!(
            is(&editor, 9, -4, 12, LINING),
            "walls reach the deeper floor"
        );
    }
}
