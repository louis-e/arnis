//! Swimming pools: a level basin lined in tile, rimmed with coping, with lane lines in
//! long rectangular pools.

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::sport_pitches::longest_edge_dir;
use crate::floodfill_cache::{is_oversized_ring, BuildingFootprintBitmap, FloodFillCache};
use crate::ground_decoration::{LOOSE_PLANTS, STACKED_PLANT_PARTS};
use crate::osm_parser::ProcessedWay;
use crate::water_depth::{clear_tree_from, is_tree_part};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const COPING: Block = SMOOTH_QUARTZ;
const LINING: Block = LIGHT_BLUE_CONCRETE;
const LANE_LINE: Block = BLUE_CONCRETE;

const DEFAULT_DEPTH: i32 = 2;
const MAX_DEPTH: i32 = 5;
/// Pools smaller than this are plunge pools and tubs, one block deep.
const PLUNGE_POOL_CELLS: usize = 12;

/// Ground the coping may replace. Paths, walls and anything else around the pool stay.
const COPING_REPLACES: &[Block] = &[GRASS_BLOCK, DIRT, COARSE_DIRT, PODZOL, MUD, SAND, GRAVEL];

pub fn is_swimming_pool(tags: &HashMap<String, String>) -> bool {
    tags.get("leisure").map(String::as_str) == Some("swimming_pool")
}

pub fn generate_swimming_pool(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    timeout: Option<&Duration>,
) {
    let closed = way.nodes.len() >= 4
        && way.nodes.first().map(|n| (n.x, n.z)) == way.nodes.last().map(|n| (n.x, n.z));
    if !closed {
        return;
    }
    let fill = flood_fill_cache.get_or_compute(way, timeout);
    if fill.is_empty() && is_oversized_ring(way) {
        return;
    }

    // The mapped ring is the water's edge, so its own cells hold water too.
    let mut cells: HashSet<(i32, i32)> = fill.iter().copied().collect();
    for pair in way.nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            cells.insert((x, z));
        }
    }
    // An indoor pool stays under its hall's floor.
    cells.retain(|&(x, z)| !building_footprints.contains(x, z));
    let Some(water_y) = cells
        .iter()
        .map(|&(x, z)| editor.get_ground_level(x, z))
        .max()
    else {
        return;
    };

    // The surface sits at the highest ground under the pool, so on a slope the low end
    // stands on a wall instead of the high end being buried.
    let depth = pool_depth(&way.tags, cells.len());
    let floor_y = water_y - depth;
    let lanes = lane_cells(way, &cells, editor.scale());

    for &(x, z) in &cells {
        let ground_y = editor.get_ground_level(x, z);
        for y in floor_y + 1..=water_y {
            editor.set_block_absolute(WATER, x, y, z, None, Some(&[]));
        }
        let floor = if lanes.contains(&(x, z)) {
            LANE_LINE
        } else {
            LINING
        };
        editor.set_block_absolute(floor, x, floor_y, z, None, Some(&[]));
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
        .filter(|c| !cells.contains(c) && !building_footprints.contains(c.0, c.1))
        .collect();
    rim.sort_unstable();
    rim.dedup();

    for (x, z) in rim {
        let ground_y = editor.get_ground_level(x, z);
        let top = water_y.max(ground_y);
        editor.set_block_absolute(COPING, x, top, z, Some(COPING_REPLACES), None);
        // Coping below the top where the wall stands clear of the ground, tile underneath.
        if ground_y < top - 1 {
            editor.fill_column_absolute(COPING, x, z, ground_y + 1, top - 1, true);
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

/// Water depth in blocks: `depth=*` in metres when tagged, else from the pool's kind.
fn pool_depth(tags: &HashMap<String, String>, cells: usize) -> i32 {
    let tagged = tags
        .get("depth")
        .and_then(|d| d.trim().trim_end_matches('m').trim().parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    if let Some(d) = tagged {
        return (d.round() as i32).clamp(1, MAX_DEPTH);
    }
    match tags.get("swimming_pool").map(String::as_str) {
        Some("wading" | "paddling" | "kiddie" | "toddler" | "children") => 1,
        Some("diving") => 4,
        _ if cells < PLUNGE_POOL_CELLS => 1,
        _ => DEFAULT_DEPTH,
    }
}

/// Floor cells under the lane lines of a long rectangular pool, empty for any other.
/// Each line stops short of the walls and ends in a cross bar.
fn lane_cells(way: &ProcessedWay, cells: &HashSet<(i32, i32)>, scale: f64) -> HashSet<(i32, i32)> {
    let mut lines = HashSet::new();
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
    // A freeform pool has no lanes, nor does a garden pool too short to swim laps in.
    if length < 20.0 * scale || width < 7.0 * scale || n < 0.85 * length * width {
        return lines;
    }

    let lane = 2.5 * scale;
    let count = (width / lane).floor();
    let first = (v_min + v_max) / 2.0 - count * lane / 2.0 + lane / 2.0;
    let u_mid = (u_min + u_max) / 2.0;
    let line_end = length / 2.0 - 2.0 * scale;
    // Half-open, so a line falling between two rows takes exactly one of them.
    let on = |d: f64, half: f64| d > -half && d <= half;
    for &(x, z) in cells {
        let (dx, dz) = (x as f64 - cx, z as f64 - cz);
        let u = dx * ux + dz * uz - u_mid;
        let v = dx * vx + dz * vz;
        let marked = (0..count as i32).any(|k| {
            let dv = v - (first + f64::from(k) * lane);
            (on(dv, 0.5) && u.abs() <= line_end) || (on(dv, 1.5) && on(u.abs() - line_end, 0.5))
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
    use crate::osm_parser::ProcessedNode;
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

    fn pool(x0: i32, z0: i32, x1: i32, z1: i32, tags: &[(&str, &str)]) -> ProcessedWay {
        let corners = [(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)];
        let mut all: HashMap<String, String> = tags
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        all.insert("leisure".to_string(), "swimming_pool".to_string());
        ProcessedWay {
            id: 1,
            nodes: corners
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: (i % 4) as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: all,
        }
    }

    fn generate(
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        footprints: &BuildingFootprintBitmap,
    ) {
        generate_swimming_pool(editor, way, &FloodFillCache::new(), footprints, None);
    }

    fn is(editor: &WorldEditor, x: i32, y: i32, z: i32, block: Block) -> bool {
        editor.check_for_block_absolute(x, y, z, Some(&[block]), None)
    }

    #[test]
    fn a_pool_is_a_lined_basin_rimmed_with_coping() {
        let bbox = bbox();
        let mut editor = editor(&bbox);
        let footprints = BuildingFootprintBitmap::new(&bbox);
        generate(&mut editor, &pool(10, 10, 19, 15, &[]), &footprints);

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
    }

    #[test]
    fn a_pool_on_a_slope_stays_level_and_walls_its_low_end() {
        let bbox = bbox();
        let mut editor = editor_over_slope(&bbox, 4.0, 0.25);
        let low = editor.get_ground_level(10, 12);
        let high = editor.get_ground_level(21, 12);
        assert!(high >= low + 2, "fixture no longer slopes: {low}..{high}");
        let footprints = BuildingFootprintBitmap::new(&bbox);
        generate(&mut editor, &pool(10, 10, 21, 15, &[]), &footprints);

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
    }

    #[test]
    fn a_lap_pool_gets_lane_lines_and_a_garden_pool_does_not() {
        let bbox = bbox();
        let footprints = BuildingFootprintBitmap::new(&bbox);

        let mut lap = editor(&bbox);
        generate(&mut lap, &pool(5, 5, 29, 16, &[]), &footprints);
        // 25 by 12 holds four lanes, so four lines run along the length.
        let lines: Vec<i32> = (5..=16)
            .filter(|&z| is(&lap, 17, -2, z, LANE_LINE))
            .collect();
        assert_eq!(lines.len(), 4, "lane lines at {lines:?}");
        assert!(
            !is(&lap, 5, -2, lines[0], LANE_LINE),
            "lines stop short of the wall"
        );
        let bar = (5..=29).find(|&x| is(&lap, x, -2, lines[0] - 1, LANE_LINE));
        assert!(bar.is_some(), "line ends without a cross bar");

        let mut garden = editor(&bbox);
        generate(&mut garden, &pool(5, 5, 14, 9, &[]), &footprints);
        for x in 5..=14 {
            for z in 5..=9 {
                assert!(!is(&garden, x, -2, z, LANE_LINE), "lane at ({x}, {z})");
            }
        }
    }

    #[test]
    fn an_indoor_pool_leaves_its_hall_alone() {
        let bbox = bbox();
        let mut editor = editor(&bbox);
        let mut footprints = BuildingFootprintBitmap::new(&bbox);
        for x in 8..=21 {
            for z in 8..=17 {
                footprints.set(x, z);
            }
        }
        generate(&mut editor, &pool(10, 10, 19, 15, &[]), &footprints);
        for x in 0..SIDE as i32 {
            for z in 0..SIDE as i32 {
                assert!(!editor.block_exists_absolute(x, 0, z), "drew at ({x}, {z})");
            }
        }
    }

    #[test]
    fn coping_does_not_pave_over_a_path() {
        let bbox = bbox();
        let mut editor = editor(&bbox);
        editor.set_block_absolute(GRAY_CONCRETE, 9, 0, 12, None, None);
        editor.set_block_absolute(GRASS_BLOCK, 9, 0, 13, None, None);
        let footprints = BuildingFootprintBitmap::new(&bbox);
        generate(&mut editor, &pool(10, 10, 19, 15, &[]), &footprints);
        assert!(is(&editor, 9, 0, 12, GRAY_CONCRETE));
        assert!(is(&editor, 9, 0, 13, COPING));
        assert!(
            is(&editor, 9, -1, 12, LINING),
            "the wall still runs under the path"
        );
    }

    #[test]
    fn depth_follows_the_tags_and_the_size() {
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(pool_depth(&tags(&[]), 60), DEFAULT_DEPTH);
        assert_eq!(pool_depth(&tags(&[]), 6), 1);
        assert_eq!(pool_depth(&tags(&[("depth", "3.5 m")]), 60), 4);
        assert_eq!(pool_depth(&tags(&[("depth", "40")]), 60), MAX_DEPTH);
        assert_eq!(pool_depth(&tags(&[("depth", "deep")]), 60), DEFAULT_DEPTH);
        assert_eq!(pool_depth(&tags(&[("swimming_pool", "wading")]), 60), 1);
        assert_eq!(pool_depth(&tags(&[("swimming_pool", "diving")]), 60), 4);

        let bbox = bbox();
        let mut editor = editor(&bbox);
        let footprints = BuildingFootprintBitmap::new(&bbox);
        generate(
            &mut editor,
            &pool(10, 10, 19, 15, &[("depth", "4")]),
            &footprints,
        );
        assert!(is(&editor, 14, -3, 12, WATER));
        assert!(is(&editor, 14, -4, 12, LINING));
        assert!(
            is(&editor, 9, -4, 12, LINING),
            "walls reach the deeper floor"
        );
    }
}
