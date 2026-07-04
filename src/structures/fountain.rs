//! Bundled fountains at OSM amenity=fountain; fountain 4 (large) only for big polygons.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: [&[u8]; 4] = [
    include_bytes!("../../assets/structures/fountain1.schem"),
    include_bytes!("../../assets/structures/fountain2.schem"),
    include_bytes!("../../assets/structures/fountain3.schem"),
    include_bytes!("../../assets/structures/fountain4.schem"),
];

/// Footprint (cells) at/above which a fountain is "large" and gets fountain 4.
const LARGE_FOUNTAIN_CELLS: usize = 300;

fn variants() -> &'static [Option<StructureSchematic>; 4] {
    static CELL: OnceLock<[Option<StructureSchematic>; 4]> = OnceLock::new();
    CELL.get_or_init(|| {
        std::array::from_fn(|i| match load_structure(BYTES[i]) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("fountain{} load failed: {e}", i + 1);
                None
            }
        })
    })
}

/// Stamp a fountain at (x, z); `area_cells` is footprint size (0 for a node), large areas get fountain 4 else a random small one (1-3).
pub fn place(editor: &mut WorldEditor, x: i32, z: i32, area_cells: usize) {
    if !editor.place_schematics() {
        return;
    }
    let variants = variants();
    let h = coord_hash(x, z);
    let pick = if area_cells >= LARGE_FOUNTAIN_CELLS {
        3
    } else {
        (h >> 4) as usize % 3
    };
    // Fall back to any loaded small fountain if the chosen one failed to parse.
    let schem = match variants[pick].as_ref() {
        Some(s) => s,
        None => match variants[..3].iter().flatten().next() {
            Some(s) => s,
            None => return,
        },
    };
    let rot = ((h >> 8) & 3) as u8;
    let base_y = editor.get_absolute_y(x, 1, z);
    place_structure(editor, schem, x, z, base_y, rot, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fountain_assets_parse() {
        let v = variants();
        for (i, s) in v.iter().enumerate() {
            let s = s
                .as_ref()
                .unwrap_or_else(|| panic!("fountain{} failed", i + 1));
            assert!(!s.voxels.is_empty(), "fountain{} empty", i + 1);
        }
    }
}
