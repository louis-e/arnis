//! Bundled tombstones scattered across landuse=cemetery areas.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

// Small headstones, footprint <= 4.
static SMALL_BYTES: &[&[u8]] = &[
    include_bytes!("../../assets/structures/tombstone1.schem"),
    include_bytes!("../../assets/structures/tombstone2.schem"),
    include_bytes!("../../assets/structures/tombstone3.schem"),
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
// Crypt footprint half-extent, used to keep headstones clear of crypts.
const LARGE_HALF: i32 = 6;
const LARGE_PERCENT: u64 = 5;
// Headstone grid spacing equals their max footprint, so neighbours touch but don't overlap.
const SMALL_GRID: i32 = 4;
const SMALL_PERCENT: u64 = 28;

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

fn small() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| parse_all(SMALL_BYTES))
}

fn large() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| parse_all(LARGE_BYTES))
}

/// Stamp at most one tombstone at cemetery cell (x, z), keyed on coord_hash for seam stability.
pub fn maybe_place(editor: &mut WorldEditor, x: i32, z: i32, road_mask: &RoadMaskBitmap) {
    // Skip water and roads; both reuse existing data, no extra memory.
    if editor.is_lc_water(x, z) || road_mask.contains(x, z) {
        return;
    }

    // Crypts on the coarse grid first; a crypt cell never also gets a headstone.
    if x.rem_euclid(LARGE_GRID) == 0 && z.rem_euclid(LARGE_GRID) == 0 {
        let h = coord_hash(x, z);
        if h % 100 < LARGE_PERCENT {
            if let Some(schem) = pick(large(), h) {
                let base_y = editor.get_absolute_y(x, 0, z);
                place_structure(editor, schem, x, z, base_y, ((h >> 16) & 3) as u8, None);
            }
            return;
        }
    }

    // Headstones on the fine grid, skipping any cell a crypt already occupies.
    if x.rem_euclid(SMALL_GRID) == 0 && z.rem_euclid(SMALL_GRID) == 0 && !near_large_crypt(x, z) {
        let h = coord_hash(x.wrapping_add(0x5f5f), z.wrapping_add(0x3c3c));
        if h % 100 < SMALL_PERCENT {
            if let Some(schem) = pick(small(), h) {
                let base_y = editor.get_absolute_y(x, 0, z);
                place_structure(editor, schem, x, z, base_y, ((h >> 16) & 3) as u8, None);
            }
        }
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
fn near_large_crypt(x: i32, z: i32) -> bool {
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
        assert_eq!(small().len(), 9, "all small headstones should parse");
        assert_eq!(large().len(), 2, "both crypts should parse");
        for s in small().iter().chain(large()) {
            assert!(!s.voxels.is_empty());
        }
    }
}
