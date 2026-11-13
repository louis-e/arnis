//! Bundled grave models: crypts, graves laid in rows, and crosses over mapped graves.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

// Tall crosses on a 4x3 base, for graves mapped one by one.
static CROSS_BYTES: &[&[u8]] = &[
    include_bytes!("../../assets/structures/tombstone1.schem"),
    include_bytes!("../../assets/structures/tombstone2.schem"),
    include_bytes!("../../assets/structures/tombstone3.schem"),
];
// Graves 3 long on +x: headstone, sign and flower pot.
static GRAVE_BYTES: &[&[u8]] = &[
    include_bytes!("../../assets/structures/tombstone4.schem"),
    include_bytes!("../../assets/structures/tombstone5.schem"),
    include_bytes!("../../assets/structures/tombstone6.schem"),
    include_bytes!("../../assets/structures/tombstone7.schem"),
    include_bytes!("../../assets/structures/tombstone8.schem"),
    include_bytes!("../../assets/structures/tombstone9.schem"),
];
// Large crypts, footprint ~10x7.
static LARGE_BYTES: &[&[u8]] = &[
    include_bytes!("../../assets/structures/tombstone10.schem"),
    include_bytes!("../../assets/structures/tombstone11.schem"),
];

// Wide crypt grid plus low percent keeps the big mausoleums rare and non-overlapping.
const LARGE_GRID: i32 = 32;
// Crypt footprint half-extent, used to keep graves clear of crypts.
const LARGE_HALF: i32 = 6;
const LARGE_PERCENT: u64 = 5;

fn parse_all(raw: &'static [&'static [u8]]) -> Vec<StructureSchematic> {
    raw.iter()
        .filter_map(|b| match load_structure(b) {
            Ok(s) => Some(s.centered()),
            Err(e) => {
                eprintln!("tombstone schem load failed: {e}");
                None
            }
        })
        .collect()
}

fn crosses() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| parse_all(CROSS_BYTES))
}

fn graves() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| parse_all(GRAVE_BYTES))
}

fn large() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| parse_all(LARGE_BYTES))
}

/// Stamps a crypt if (x, z) is one of the rare crypt nodes; keyed on coord_hash for seam stability.
pub fn maybe_place_crypt(editor: &mut WorldEditor, x: i32, z: i32, road_mask: &RoadMaskBitmap) {
    if !editor.place_prop(super::Prop::Tombstone)
        || x.rem_euclid(LARGE_GRID) != 0
        || z.rem_euclid(LARGE_GRID) != 0
        || editor.is_lc_water(x, z)
        || road_mask.contains(x, z)
    {
        return;
    }
    let h = coord_hash(x, z);
    if h % 100 < LARGE_PERCENT {
        if let Some(schem) = pick(large(), h) {
            let base_y = editor.get_absolute_y(x, 0, z);
            place_structure(editor, schem, x, z, base_y, ((h >> 16) & 3) as u8, None);
        }
    }
}

/// A grave model from `head` along the unit step `dir`; false when schematics are off.
pub fn place_grave(editor: &mut WorldEditor, head: (i32, i32), dir: (i32, i32), h: u64) -> bool {
    place_model(editor, false, (head.0 + dir.0, head.1 + dir.1), dir, h)
}

/// A model at its centre `anchor`, its length along `dir`: a 3x1 grave, or a 4x3 cross grave.
pub fn place_model(
    editor: &mut WorldEditor,
    cross: bool,
    anchor: (i32, i32),
    dir: (i32, i32),
    h: u64,
) -> bool {
    if !editor.place_prop(super::Prop::Tombstone) {
        return false;
    }
    let Some(schem) = pick(if cross { crosses() } else { graves() }, h) else {
        return false;
    };
    // Quarter turns take the model's +x to east, south, west and north.
    let rot = match dir {
        (1, _) => 0,
        (0, 1) => 1,
        (-1, _) => 2,
        _ => 3,
    };
    let base_y = editor.get_absolute_y(anchor.0, 0, anchor.1);
    place_structure(editor, schem, anchor.0, anchor.1, base_y, rot, None);
    true
}

/// A cross over a grave mapped on its own.
pub fn place_cross(editor: &mut WorldEditor, x: i32, z: i32) {
    if !editor.place_prop(super::Prop::Tombstone) {
        return;
    }
    let h = coord_hash(x.wrapping_add(0x5f5f), z.wrapping_add(0x3c3c));
    if let Some(schem) = pick(crosses(), h) {
        let base_y = editor.get_absolute_y(x, 0, z);
        place_structure(editor, schem, x, z, base_y, ((h >> 16) & 3) as u8, None);
    }
}

fn pick(models: &[StructureSchematic], h: u64) -> Option<&StructureSchematic> {
    if models.is_empty() {
        None
    } else {
        Some(&models[(h >> 8) as usize % models.len()])
    }
}

/// True if the nearest crypt node hosts a crypt covering (x, z). Replays the spawn test, no state.
pub fn near_crypt(x: i32, z: i32) -> bool {
    let gx = (x as f64 / LARGE_GRID as f64).round() as i32 * LARGE_GRID;
    let gz = (z as f64 / LARGE_GRID as f64).round() as i32 * LARGE_GRID;
    coord_hash(gx, gz) % 100 < LARGE_PERCENT
        && (x - gx).abs() <= LARGE_HALF
        && (z - gz).abs() <= LARGE_HALF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tombstones_parse() {
        assert_eq!(crosses().len(), 3, "all crosses should parse");
        assert_eq!(graves().len(), 6, "all grave models should parse");
        assert_eq!(large().len(), 2, "both crypts should parse");
        for s in crosses().iter().chain(graves()).chain(large()) {
            assert!(!s.voxels.is_empty());
        }
    }
}
