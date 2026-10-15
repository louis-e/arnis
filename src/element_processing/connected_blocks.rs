//! Walls, iron bars and fences with their side connections spelled out. Chunks keep
//! stored blockstates until a neighbour update, so defaults render as separate posts.

use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;

/// Neighbour offsets as (dx, dz): north, south, east, west.
const SIDES: [(i32, i32); 4] = [(0, -1), (0, 1), (1, 0), (-1, 0)];

/// Which run a block joins: walls and bars join each other, fences join fences.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    WallOrBars,
    Fence,
}

fn family(block: Block) -> Option<Family> {
    let name = block.name();
    if block == IRON_BARS || name.ends_with("_wall") {
        Some(Family::WallOrBars)
    } else if name.ends_with("_fence") {
        Some(Family::Fence)
    } else {
        None
    }
}

/// True for the blocks `place_connected` knows how to join.
pub(crate) fn is_connectable(block: Block) -> bool {
    family(block).is_some()
}

/// Places a wall, bar or fence (into an empty cell) joined to its neighbours, updating them too.
pub(crate) fn place_connected(editor: &mut WorldEditor, block: Block, x: i32, y: i32, z: i32) {
    editor.set_block_absolute(block, x, y, z, None, None);
    if editor.get_block_absolute(x, y, z) != Some(block) || family(block).is_none() {
        return;
    }
    refresh(editor, block, x, y, z);
    for (dx, dz) in SIDES {
        if let Some(neighbour) = editor.get_block_absolute(x + dx, y, z + dz) {
            if family(neighbour) == family(block) {
                refresh(editor, neighbour, x + dx, y, z + dz);
            }
        }
    }
}

fn refresh(editor: &mut WorldEditor, block: Block, x: i32, y: i32, z: i32) {
    let own = family(block);
    let [north, south, east, west] = SIDES.map(|(dx, dz)| {
        editor
            .get_block_absolute(x + dx, y, z + dz)
            .is_some_and(|b| family(b) == own)
    });
    let joined = if block == IRON_BARS {
        connected_iron_bars(north, south, east, west)
    } else if own == Some(Family::Fence) {
        connected_fence(block, north, south, east, west)
    } else {
        connected_wall(block, north, south, east, west)
    };
    editor.set_block_with_properties_absolute(joined, x, y, z, Some(&[block]), None);
}

fn flag(v: bool) -> Value {
    Value::String(if v { "true" } else { "false" }.to_string())
}

/// Iron bars joined to the given sides.
pub(crate) fn connected_iron_bars(
    north: bool,
    south: bool,
    east: bool,
    west: bool,
) -> BlockWithProperties {
    connected_fence(IRON_BARS, north, south, east, west)
}

/// A fence or bars joined to the given sides.
fn connected_fence(
    block: Block,
    north: bool,
    south: bool,
    east: bool,
    west: bool,
) -> BlockWithProperties {
    BlockWithProperties::new(
        block,
        Some(Value::Compound(HashMap::from([
            ("north".to_string(), flag(north)),
            ("south".to_string(), flag(south)),
            ("east".to_string(), flag(east)),
            ("west".to_string(), flag(west)),
            ("waterlogged".to_string(), flag(false)),
        ]))),
    )
}

/// A wall joined to the given sides; straight runs hide the post.
fn connected_wall(
    block: Block,
    north: bool,
    south: bool,
    east: bool,
    west: bool,
) -> BlockWithProperties {
    let side = |v: bool| Value::String(if v { "low" } else { "none" }.to_string());
    let straight = (north && south && !east && !west) || (east && west && !north && !south);
    BlockWithProperties::new(
        block,
        Some(Value::Compound(HashMap::from([
            ("north".to_string(), side(north)),
            ("south".to_string(), side(south)),
            ("east".to_string(), side(east)),
            ("west".to_string(), side(west)),
            ("up".to_string(), flag(!straight)),
            ("waterlogged".to_string(), flag(false)),
        ]))),
    )
}

/// Cells from `prev` (exclusive) to `curr` (inclusive) in orthogonal steps.
pub(crate) fn stair_steps(prev: (i32, i32), curr: (i32, i32)) -> Vec<(i32, i32)> {
    let mut cells = Vec::with_capacity(2);
    let (mut x, mut z) = prev;
    while x != curr.0 || z != curr.1 {
        if x != curr.0 {
            x += (curr.0 - x).signum();
            cells.push((x, z));
        }
        if z != curr.1 {
            z += (curr.1 - z).signum();
            cells.push((x, z));
        }
    }
    if cells.is_empty() {
        cells.push(curr);
    }
    cells
}

/// A polyline's cells with diagonal steps split into orthogonal ones.
pub(crate) fn four_connected_line(points: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(points.len() * 2);
    for &p in points {
        match out.last() {
            Some(&last) if last == p => {}
            Some(&last) => out.extend(stair_steps(last, p)),
            None => out.push(p),
        }
    }
    out
}

/// Cells across a deck at centerline (x, z), from offset `from` to `to` along `perp`.
pub(crate) fn cross_cells(x: i32, z: i32, perp: (f32, f32), from: i32, to: i32) -> Vec<(i32, i32)> {
    let points: Vec<(i32, i32)> = (from..=to)
        .map(|o| {
            (
                (x as f32 + perp.0 * o as f32).round() as i32,
                (z as f32 + perp.1 * o as f32).round() as i32,
            )
        })
        .collect();
    four_connected_line(&points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use std::path::PathBuf;

    fn props_of(editor: &WorldEditor, x: i32, y: i32, z: i32) -> HashMap<String, String> {
        let Some(Value::Compound(map)) = editor.block_properties_absolute(x, y, z) else {
            return HashMap::new();
        };
        map.into_iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) => Some((k, s)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_wall_run_joins_whichever_end_is_placed_first() {
        let xzbbox = XZBBox::rect_from_xz_lengths(20.0, 20.0).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), &xzbbox, llbbox);
        for x in [5, 7, 6] {
            place_connected(&mut editor, BRICK_WALL, x, 3, 5);
        }
        let middle = props_of(&editor, 6, 3, 5);
        assert_eq!(middle["east"], "low");
        assert_eq!(middle["west"], "low");
        assert_eq!(middle["north"], "none");
        assert_eq!(middle["up"], "false", "a straight run hides its post");
        let end = props_of(&editor, 5, 3, 5);
        assert_eq!(
            end["east"], "low",
            "placed before its neighbour, still joined"
        );
        assert_eq!(end["west"], "none");
        assert_eq!(end["up"], "true");
    }

    #[test]
    fn fences_join_fences_but_not_walls() {
        let xzbbox = XZBBox::rect_from_xz_lengths(20.0, 20.0).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), &xzbbox, llbbox);
        place_connected(&mut editor, OAK_FENCE, 5, 3, 5);
        place_connected(&mut editor, OAK_FENCE, 5, 3, 6);
        place_connected(&mut editor, STONE_BRICK_WALL, 6, 3, 5);
        let fence = props_of(&editor, 5, 3, 5);
        assert_eq!(fence["south"], "true");
        assert_eq!(fence["east"], "false");
    }

    #[test]
    fn diagonal_steps_become_orthogonal_ones() {
        let cells = four_connected_line(&[(0, 0), (1, 1), (2, 1), (2, 1), (3, 2)]);
        assert_eq!(cells, vec![(0, 0), (1, 0), (1, 1), (2, 1), (3, 1), (3, 2)]);
        for pair in cells.windows(2) {
            let d = (pair[0].0 - pair[1].0).abs() + (pair[0].1 - pair[1].1).abs();
            assert_eq!(d, 1);
        }
    }
}
