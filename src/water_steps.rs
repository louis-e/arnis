//! Flowing water on the run-up to a one-block drop in a water surface, so it reads as a slope.

use std::collections::VecDeque;

use fastnbt::Value;
use fnv::{FnvHashMap, FnvHashSet};
use rayon::prelude::*;

use crate::block_definitions::{
    BlockWithProperties, BRAIN_CORAL, BRAIN_CORAL_FAN, BUBBLE_CORAL, BUBBLE_CORAL_FAN, FIRE_CORAL,
    FIRE_CORAL_FAN, HORN_CORAL, HORN_CORAL_FAN, KELP, KELP_PLANT, SEAGRASS, SEA_PICKLE,
    TALL_SEAGRASS_BOTTOM, TALL_SEAGRASS_TOP, TUBE_CORAL, TUBE_CORAL_FAN, WATER,
};
use crate::element_processing::structure_kit::with_prop;
use crate::world_editor::common::{ChunkToModify, SectionToModify};
use crate::world_editor::WorldEditor;

/// Furthest vanilla water spreads from a source, so the longest run-up.
const MAX_LEVEL: u8 = 7;
/// Columns a decision reads around it: fill 7, run-up check 1, run-up 7, drop 1, plus slack.
const MARGIN: i32 = 20;
/// Top water this far under the water level is a cave pool, not a surface.
const CAVE_DEPTH: i32 = 5;
/// Wet blocks counted under a top; a deeper run only has to read as wet.
const DEPTH_CAP: u8 = 16;
const REGION: i32 = 512;

/// Air on top of the column's water.
const OPEN: u8 = 1;
/// Open, plain, floored water that may flow.
const FREE: u8 = 2;

/// Unassigned run-up cell during the level fill.
const PENDING: u8 = u8::MAX;

/// The topmost wet run of a column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Col {
    top: i16,
    depth: u8,
    flags: u8,
}

impl Col {
    const DRY: Col = Col {
        top: i16::MIN,
        depth: 0,
        flags: 0,
    };

    #[inline]
    fn wet_at(self, y: i16) -> bool {
        y <= self.top && i32::from(self.top) - i32::from(y) < i32::from(self.depth)
    }

    #[inline]
    fn free(self) -> bool {
        self.flags & FREE != 0
    }
}

/// Water, and what stands in it: sea plants, corals and anything waterlogged.
#[inline]
fn is_wet(section: &SectionToModify, idx: usize) -> bool {
    matches!(
        section.storage.get(idx),
        WATER
            | SEAGRASS
            | TALL_SEAGRASS_BOTTOM
            | TALL_SEAGRASS_TOP
            | KELP
            | KELP_PLANT
            | SEA_PICKLE
            | TUBE_CORAL
            | BRAIN_CORAL
            | BUBBLE_CORAL
            | FIRE_CORAL
            | HORN_CORAL
            | TUBE_CORAL_FAN
            | BRAIN_CORAL_FAN
            | BUBBLE_CORAL_FAN
            | FIRE_CORAL_FAN
            | HORN_CORAL_FAN
    ) || (!section.properties.is_empty()
        && section.properties.get(&idx).is_some_and(|p| {
            matches!(p.as_ref(), Value::Compound(m)
                if matches!(m.get("waterlogged"), Some(Value::String(v)) if v == "true"))
        }))
}

/// Whether the block at `y` in a column holds water.
fn wet_at(chunk: &ChunkToModify, lx: u8, y: i32, lz: u8) -> bool {
    let Some(section) = i8::try_from(y >> 4)
        .ok()
        .and_then(|sy| chunk.sections.get(sy))
    else {
        return false;
    };
    is_wet(section, SectionToModify::index(lx, (y & 15) as u8, lz))
}

/// Inclusive block rectangle.
#[derive(Clone, Copy)]
struct Rect {
    x0: i32,
    z0: i32,
    x1: i32,
    z1: i32,
}

impl Rect {
    fn grow(self, by: i32) -> Rect {
        Rect {
            x0: self.x0 - by,
            z0: self.z0 - by,
            x1: self.x1 + by,
            z1: self.z1 + by,
        }
    }

    fn clip(self, o: Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(o.x0),
            z0: self.z0.max(o.z0),
            x1: self.x1.min(o.x1),
            z1: self.z1.min(o.z1),
        };
        (r.x0 <= r.x1 && r.z0 <= r.z1).then_some(r)
    }

    fn width(self) -> usize {
        (self.x1 - self.x0 + 1) as usize
    }

    fn height(self) -> usize {
        (self.z1 - self.z0 + 1) as usize
    }
}

/// Pre-shaping border columns of shaped regions, for neighbours shaped after they are written out.
#[derive(Default)]
pub(crate) struct Borders {
    saved: FnvHashMap<(i32, i32), Border>,
    shaped: FnvHashSet<(i32, i32)>,
}

/// Shapes every region of the editor's area, all resident and untouched since ground generation.
pub fn shape_all(editor: &mut WorldEditor, caves: bool) {
    let area = area(editor);
    let mut regions = Vec::new();
    for rz in (area.z0 >> 9)..=(area.z1 >> 9) {
        for rx in (area.x0 >> 9)..=(area.x1 >> 9) {
            regions.push((rx, rz));
        }
    }
    let plans: Vec<Writes> = {
        let editor = &*editor;
        regions
            .par_iter()
            .map(|&(rx, rz)| plan(editor, rx, rz, caves, None).0)
            .collect()
    };
    apply(editor, plans.iter().flatten());
}

/// Shapes region (rx, rz) once its 3x3 neighbourhood has merged, before anything is placed on it.
pub fn shape_region(
    editor: &mut WorldEditor,
    (rx, rz): (i32, i32),
    caves: bool,
    borders: &mut Borders,
    exists: impl Fn((i32, i32)) -> bool,
) {
    let (writes, border) = plan(editor, rx, rz, caves, Some(&borders.saved));
    apply(editor, writes.iter());
    borders.shaped.insert((rx, rz));
    borders.saved.insert((rx, rz), border);
    // A region's border is read only by neighbours still to be shaped.
    for dz in -1..=1 {
        for dx in -1..=1 {
            let r = (rx + dx, rz + dz);
            if !borders.saved.contains_key(&r) {
                continue;
            }
            let settled = (-1..=1).all(|nz| {
                (-1..=1).all(|nx| {
                    let n = (r.0 + nx, r.1 + nz);
                    n == r || !exists(n) || borders.shaped.contains(&n)
                })
            });
            if settled {
                borders.saved.remove(&r);
            }
        }
    }
}

fn area(editor: &WorldEditor) -> Rect {
    let (x0, z0) = editor.get_min_coords();
    let (x1, z1) = editor.get_max_coords();
    Rect { x0, z0, x1, z1 }
}

type Writes = Vec<(i32, i32, i32, u8)>;
/// Wet columns within `MARGIN` of a region's edges, keyed by their index in the region.
type Border = Vec<(u32, Col)>;

/// The flowing blocks of one region, and its border columns for later neighbours.
fn plan(
    editor: &WorldEditor,
    rx: i32,
    rz: i32,
    caves: bool,
    saved: Option<&FnvHashMap<(i32, i32), Border>>,
) -> (Writes, Border) {
    let area = area(editor);
    let region = Rect {
        x0: rx * REGION,
        z0: rz * REGION,
        x1: rx * REGION + REGION - 1,
        z1: rz * REGION + REGION - 1,
    };
    let Some(rect) = region.clip(area) else {
        return (Writes::new(), Border::new());
    };
    let win = rect.grow(MARGIN).clip(area).expect("window holds its rect");
    let (w, h) = (win.width(), win.height());
    let mut cols = scan(editor, win, area, caves, |r| {
        r != (rx, rz) && saved.is_some_and(|s| s.contains_key(&r))
    });
    let neighbours = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (rx + dx, rz + dz)));
    for r in neighbours.filter(|&r| r != (rx, rz)) {
        let Some(border) = saved.and_then(|s| s.get(&r)) else {
            continue;
        };
        let (ox, oz) = (r.0 * REGION, r.1 * REGION);
        for &(idx, col) in border {
            let x = ox + (idx % REGION as u32) as i32;
            let z = oz + (idx / REGION as u32) as i32;
            if x >= win.x0 && x <= win.x1 && z >= win.z0 && z <= win.z1 {
                cols[(z - win.z0) as usize * w + (x - win.x0) as usize] = col;
            }
        }
    }

    let mut writes = Writes::new();
    for (i, level) in levels(&cols, w, h) {
        let (x, z) = (win.x0 + (i % w) as i32, win.z0 + (i / w) as i32);
        if x >= rect.x0 && x <= rect.x1 && z >= rect.z0 && z <= rect.z1 {
            writes.push((x, i32::from(cols[i].top), z, level));
        }
    }
    let mut border = Border::new();
    if saved.is_some() {
        for z in rect.z0..=rect.z1 {
            let row = (z - win.z0) as usize * w;
            let edge_z = z - region.z0 < MARGIN || region.z1 - z < MARGIN;
            for x in rect.x0..=rect.x1 {
                let col = cols[row + (x - win.x0) as usize];
                if col.depth > 0 && (edge_z || x - region.x0 < MARGIN || region.x1 - x < MARGIN) {
                    border.push((((z - region.z0) * REGION + x - region.x0) as u32, col));
                }
            }
        }
        border.shrink_to_fit();
    }
    (writes, border)
}

fn apply<'a>(editor: &mut WorldEditor, writes: impl Iterator<Item = &'a (i32, i32, i32, u8)>) {
    let flowing: Vec<BlockWithProperties> = (0..=MAX_LEVEL)
        .map(|level| with_prop(WATER, "level", &level.to_string()))
        .collect();
    for &(x, y, z, level) in writes {
        editor.set_block_with_properties_absolute(
            flowing[usize::from(level)].clone(),
            x,
            y,
            z,
            Some(&[WATER]),
            None,
        );
    }
}

/// The window's columns, row-major. Regions `skip` accepts are left dry for the caller to fill.
fn scan(
    editor: &WorldEditor,
    win: Rect,
    area: Rect,
    caves: bool,
    skip: impl Fn((i32, i32)) -> bool + Sync,
) -> Vec<Col> {
    let w = win.width();
    let mut cols = vec![Col::DRY; w * win.height()];
    let mut bands: Vec<(i32, &mut [Col])> = Vec::new();
    let mut rest = cols.as_mut_slice();
    for cz in (win.z0 >> 4)..=(win.z1 >> 4) {
        let rows = ((cz << 4) + 15).min(win.z1) - (cz << 4).max(win.z0) + 1;
        let (band, tail) = rest.split_at_mut(rows as usize * w);
        bands.push((cz, band));
        rest = tail;
    }
    bands.into_par_iter().for_each(|(cz, band)| {
        let z0 = (cz << 4).max(win.z0);
        let mut wet_sections: Vec<i8> = Vec::new();
        for cx in (win.x0 >> 4)..=(win.x1 >> 4) {
            if skip((cx >> 5, cz >> 5)) {
                continue;
            }
            let Some(chunk) = editor.chunk(cx, cz) else {
                continue;
            };
            wet_sections.clear();
            wet_sections.extend(
                chunk
                    .sections
                    .iter()
                    .filter(|(_, s)| s.storage.may_contain(WATER))
                    .map(|(y, _)| y),
            );
            if wet_sections.is_empty() {
                continue;
            }
            let x0 = (cx << 4).max(win.x0);
            let x1 = ((cx << 4) + 15).min(win.x1);
            for z in z0..z0 + (band.len() / w) as i32 {
                for x in x0..=x1 {
                    let mut col = column(chunk, &wet_sections, (x & 15) as u8, (z & 15) as u8);
                    if col.depth == 0 {
                        continue;
                    }
                    // Caves keep below the terrain, so only they put a top that low.
                    if caves && i32::from(col.top) < editor.get_water_level(x, z) - CAVE_DEPTH {
                        continue;
                    }
                    // Water beyond the area is unknown, so its edge columns stay sources.
                    if x <= area.x0 || x >= area.x1 || z <= area.z0 || z >= area.z1 {
                        col.flags &= !FREE;
                    }
                    band[(z - z0) as usize * w + (x - win.x0) as usize] = col;
                }
            }
        }
    });
    cols
}

/// Topmost wet run in one column, searched through the sections that hold water.
fn column(chunk: &ChunkToModify, wet_sections: &[i8], lx: u8, lz: u8) -> Col {
    for &sy in wet_sections.iter().rev() {
        let Some(section) = chunk.sections.get(sy) else {
            continue;
        };
        for ly in (0..16u8).rev() {
            let idx = SectionToModify::index(lx, ly, lz);
            if !is_wet(section, idx) {
                continue;
            }
            let top = i32::from(sy) * 16 + i32::from(ly);
            let mut depth = 1u8;
            while depth < DEPTH_CAP && wet_at(chunk, lx, top - i32::from(depth), lz) {
                depth += 1;
            }
            let floored =
                depth == DEPTH_CAP || chunk.get_block(lx, top - i32::from(depth), lz).is_some();
            let open = chunk.get_block(lx, top + 1, lz).is_none();
            let plain = section.storage.get(idx) == WATER && !section.properties.contains_key(&idx);
            let mut flags = 0;
            if open {
                flags |= OPEN;
                if plain && floored {
                    flags |= FREE;
                }
            }
            return Col {
                top: top as i16,
                depth,
                flags,
            };
        }
    }
    Col::DRY
}

#[inline]
fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, z) = (i % w, i / w);
    [
        (x > 0).then(|| i - 1),
        (x + 1 < w).then(|| i + 1),
        (z > 0).then(|| i - w),
        (z + 1 < h).then(|| i + w),
    ]
    .into_iter()
    .flatten()
}

/// Flowing columns and levels: run-ups of at most `MAX_LEVEL` back from a drop, fed from their far end.
fn levels(cols: &[Col], w: usize, h: usize) -> Vec<(usize, u8)> {
    // Distances are stored one up, so zeroed memory reads as unreached.
    let mut dist = vec![0u8; w * h];
    let mut reached: Vec<usize> = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        if c.free()
            && neighbours(i, w, h).any(|j| {
                i32::from(cols[j].top) == i32::from(c.top) - 1 && cols[j].flags & OPEN != 0
            })
        {
            dist[i] = 1;
            reached.push(i);
        }
    }
    if reached.is_empty() {
        return Vec::new();
    }
    let mut head = 0;
    while head < reached.len() {
        let i = reached[head];
        head += 1;
        if dist[i] > MAX_LEVEL {
            continue;
        }
        for j in neighbours(i, w, h) {
            if dist[j] == 0 && cols[j].free() && cols[j].top == cols[i].top {
                dist[j] = dist[i] + 1;
                reached.push(j);
            }
        }
    }

    let mut level = vec![0u8; w * h];
    let run_up: Vec<usize> = reached
        .into_iter()
        .filter(|&i| {
            dist[i] <= MAX_LEVEL
                && neighbours(i, w, h).any(|j| dist[j] == dist[i] + 1 && cols[j].top == cols[i].top)
        })
        .collect();
    for &i in &run_up {
        level[i] = PENDING;
    }
    let mut queue: VecDeque<usize> = run_up
        .iter()
        .copied()
        .filter(|&i| {
            let y = cols[i].top;
            neighbours(i, w, h)
                .any(|j| cols[j].wet_at(y) && (cols[j].top != y || level[j] != PENDING))
        })
        .collect();
    for &i in &queue {
        level[i] = 1;
    }
    while let Some(i) = queue.pop_front() {
        for j in neighbours(i, w, h) {
            if level[j] == PENDING && cols[j].top == cols[i].top {
                level[j] = level[i] + 1;
                queue.push_back(j);
            }
        }
    }
    // Each run-up reaches its source within MAX_LEVEL, so none is left unfed.
    debug_assert!(run_up.iter().all(|&i| level[i] <= MAX_LEVEL));
    run_up
        .into_iter()
        .filter(|&i| level[i] <= MAX_LEVEL)
        .map(|i| (i, level[i]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{LILY_PAD, STONE};
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::deterministic_rng::coord_rng;
    use rand::Rng;
    use std::sync::Arc;

    /// `levels` as one level per column.
    fn dense(cols: &[Col], w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h];
        for (i, level) in levels(cols, w, h) {
            out[i] = level;
        }
        out
    }

    fn open_water(top: i16, depth: u8) -> Col {
        Col {
            top,
            depth,
            flags: OPEN | FREE,
        }
    }

    #[test]
    fn a_step_gets_a_run_up_of_seven_levels() {
        let (w, h) = (30, 5);
        let cols: Vec<Col> = (0..w * h)
            .map(|i| open_water(if i % w < 20 { 10 } else { 9 }, 3))
            .collect();
        let levels = dense(&cols, w, h);
        for z in 0..h {
            let row: Vec<u8> = (0..w).map(|x| levels[z * w + x]).collect();
            assert_eq!(&row[11..21], &[0, 0, 1, 2, 3, 4, 5, 6, 7, 0], "row {z}");
            assert!(row[..11].iter().all(|&l| l == 0));
            assert!(row[21..].iter().all(|&l| l == 0));
        }
    }

    #[test]
    fn a_short_pool_flows_from_its_upstream_end() {
        let w = 20;
        let cols: Vec<Col> = (0..w)
            .map(|x| {
                open_water(
                    if x < 10 {
                        10
                    } else if x < 13 {
                        9
                    } else {
                        8
                    },
                    3,
                )
            })
            .collect();
        let levels = dense(&cols, w, 1);
        assert_eq!(&levels[2..13], &[0, 1, 2, 3, 4, 5, 6, 7, 0, 1, 2]);
        assert_eq!(levels[10], 0, "the short pool's upstream end feeds it");
    }

    #[test]
    fn only_one_block_drops_get_a_run_up() {
        let w = 20;
        for (lower, flows) in [(9, true), (8, false), (11, false)] {
            let cols: Vec<Col> = (0..w)
                .map(|x| open_water(if x < 10 { 10 } else { lower }, 4))
                .collect();
            assert_eq!(dense(&cols, w, 1)[9] > 0, flows, "lower surface at {lower}");
        }
        // Dry ground a block lower is a bank, not a drop.
        let cols: Vec<Col> = (0..w)
            .map(|x| if x < 10 { open_water(10, 3) } else { Col::DRY })
            .collect();
        assert!(levels(&cols, w, 1).is_empty());
    }

    #[test]
    fn covered_or_planted_water_stays_a_source() {
        let w = 20;
        let mut cols: Vec<Col> = (0..w)
            .map(|x| open_water(if x < 10 { 10 } else { 9 }, 3))
            .collect();
        cols[9].flags = 0;
        let levels = dense(&cols, w, 1);
        assert_eq!(levels[9], 0);
        assert_eq!(levels[8], 0, "the lip is gone, so nothing runs up to it");
    }

    #[test]
    fn levels_are_what_vanilla_spreading_settles_on() {
        let mut rng = coord_rng(0, 0, 0x9E37_79B9_7F4A_7C15);
        for _ in 0..200 {
            let (w, h) = (24, 24);
            let tilt = 3 + rng.random_range(0..6usize);
            let cols: Vec<Col> = (0..w * h)
                .map(|i| {
                    let (x, z) = (i % w, i / w);
                    match rng.random_range(0..20) {
                        0 => Col::DRY,
                        1 => Col {
                            top: (12 - ((x + z / 3) / tilt) as i16),
                            depth: 2,
                            flags: 0,
                        },
                        _ => open_water(
                            12 - ((x + z / 3 + rng.random_range(0..2usize)) / tilt) as i16,
                            1 + rng.random_range(0..4u8),
                        ),
                    }
                })
                .collect();
            let levels = dense(&cols, w, h);
            for i in 0..w * h {
                let level = levels[i];
                if level == 0 {
                    continue;
                }
                let y = cols[i].top;
                assert!(cols[i].free() && level <= MAX_LEVEL);
                // Vanilla: one below the highest horizontal neighbour, sources counting as level 0.
                let fed = neighbours(i, w, h)
                    .filter(|&j| cols[j].wet_at(y))
                    .map(|j| if cols[j].top == y { levels[j] } else { 0 })
                    .min();
                assert_eq!(Some(level - 1), fed, "column {i}");
            }
        }
    }

    fn editor_with<'a>(bbox: &'a XZBBox, top: impl Fn(i32, i32) -> Option<i32>) -> WorldEditor<'a> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor =
            WorldEditor::new(std::path::PathBuf::from("/dev/null/unused"), bbox, llbbox);
        for x in bbox.min_x()..=bbox.max_x() {
            for z in bbox.min_z()..=bbox.max_z() {
                let Some(t) = top(x, z) else {
                    continue;
                };
                editor.set_block_absolute(STONE, x, t - 3, z, None, None);
                for y in t - 2..=t {
                    editor.set_block_absolute(WATER, x, y, z, None, None);
                }
            }
        }
        editor
    }

    fn level_at(editor: &WorldEditor, x: i32, y: i32, z: i32) -> u8 {
        assert!(editor.check_for_block_absolute(x, y, z, Some(&[WATER]), None));
        match editor.block_properties_absolute(x, y, z) {
            Some(Value::Compound(props)) => match props.get("level") {
                Some(Value::String(l)) => l.parse().unwrap(),
                other => panic!("level {other:?}"),
            },
            None => 0,
            other => panic!("properties {other:?}"),
        }
    }

    #[test]
    fn the_editor_gets_flowing_water_at_the_lip() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 40, 30).unwrap();
        let mut editor = editor_with(&bbox, |x, _| Some(if x < 20 { 30 } else { 29 }));
        editor.set_block_absolute(LILY_PAD, 15, 31, 25, None, None);
        shape_all(&mut editor, true);
        assert_eq!(level_at(&editor, 19, 30, 10), 7);
        assert_eq!(level_at(&editor, 13, 30, 10), 1);
        assert_eq!(level_at(&editor, 12, 30, 10), 0);
        assert_eq!(level_at(&editor, 19, 29, 10), 0, "below the top");
        assert_eq!(level_at(&editor, 20, 29, 10), 0, "the lower pool");
        assert_eq!(level_at(&editor, 15, 30, 25), 0, "under a lily pad");
        assert_eq!(
            level_at(&editor, 17, 30, 25),
            1,
            "a run-up starts afresh past it"
        );
        assert_eq!(level_at(&editor, 19, 30, 0), 0, "the edge of the area");
        assert_eq!(level_at(&editor, 19, 30, 1), 1, "fed by the edge");
    }

    #[test]
    fn cave_pools_are_left_alone() {
        use crate::ground::test_support::ground_with_land_cover_and_heights;
        use crate::land_cover::LandCoverData;
        let side = 41;
        let lc = LandCoverData {
            grid: vec![vec![0u8; side]; side],
            water_distance: vec![vec![0u8; side]; side],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            width: side,
            height: side,
            cells_per_meter: 1.0,
        };
        let ground =
            ground_with_land_cover_and_heights(lc, vec![vec![60.0; side]; side], side, side);
        let bbox = XZBBox::rect_from_min_max(0, 0, 40, 40).unwrap();
        let mut editor = editor_with(&bbox, |x, z| match z {
            0..=16 => Some(if x < 20 { 60 } else { 59 }),
            20..=30 => Some(if x < 20 { 20 } else { 19 }),
            _ => None,
        });
        editor.set_ground(Arc::new(ground));
        shape_all(&mut editor, true);
        assert_eq!(level_at(&editor, 19, 60, 8), 7);
        assert_eq!(
            level_at(&editor, 19, 20, 25),
            0,
            "a pool deep under the terrain"
        );
    }

    #[test]
    fn streamed_regions_match_one_pass_across_their_seams() {
        let bbox = XZBBox::rect_from_min_max(0, 0, 1023, 40).unwrap();
        // Drops on both sides of the seam at x = 512, the run-ups crossing it.
        let top = |x: i32, z: i32| -> Option<i32> {
            if z > 30 {
                return None;
            }
            Some(
                match x {
                    ..=505 => 40,
                    506..=515 => 39,
                    516..=540 => 38,
                    _ => 37,
                } - i32::from(z > 20 && x > 520),
            )
        };
        let probe = |editor: &WorldEditor, xs: std::ops::RangeInclusive<i32>| -> Vec<u8> {
            xs.flat_map(|x| (0..=30).map(move |z| (x, z)))
                .map(|(x, z)| level_at(editor, x, top(x, z).unwrap(), z))
                .collect()
        };
        let mut whole = editor_with(&bbox, top);
        shape_all(&mut whole, false);
        let (left, right) = (probe(&whole, 460..=511), probe(&whole, 512..=580));
        assert!(left.iter().any(|&l| l > 0) && right.iter().any(|&l| l > 0));

        for order in [[(0, 0), (1, 0)], [(1, 0), (0, 0)]] {
            let mut streamed = editor_with(&bbox, top);
            let mut borders = Borders::default();
            // A placement after shaping must not reach the neighbour's run-ups.
            streamed.set_block_absolute(STONE, 505, 40, 10, Some(&[WATER]), None);
            for (rx, rz) in order {
                if rx == 1 {
                    shape_region(&mut streamed, (1, 0), false, &mut borders, |r| {
                        r.1 == 0 && (0..=1).contains(&r.0)
                    });
                    assert_eq!(probe(&streamed, 512..=580), right, "order {order:?}");
                } else {
                    streamed.set_block_absolute(WATER, 505, 40, 10, Some(&[STONE]), None);
                    shape_region(&mut streamed, (0, 0), false, &mut borders, |r| {
                        r.1 == 0 && (0..=1).contains(&r.0)
                    });
                    assert_eq!(probe(&streamed, 460..=511), left, "order {order:?}");
                    streamed.set_block_absolute(STONE, 510, 39, 15, Some(&[WATER]), None);
                }
                streamed.evict_region(rx, rz);
            }
            assert!(borders.saved.is_empty(), "both borders are released");
        }
    }
}
