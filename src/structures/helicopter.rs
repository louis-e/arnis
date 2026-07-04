//! Bundled helicopter, occasionally parked on helipads.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/helicopter.schem");

/// Chance (percent) a qualifying helipad actually gets one.
const SPAWN_PERCENT: u64 = 60;

fn helicopter() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(BYTES) {
        Ok(s) => Some(s.centered()),
        Err(e) => {
            eprintln!("helicopter load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Sometimes parks a helicopter at the pad centre; seeded by coordinates for tile-seam determinism.
pub fn maybe_place_helicopter(editor: &mut WorldEditor, cx: i32, cz: i32) {
    if !editor.place_schematics() {
        return;
    }
    let Some(schem) = helicopter() else {
        return;
    };
    let h = coord_hash(cx, cz);
    if h % 100 >= SPAWN_PERCENT {
        return;
    }
    if editor.is_lc_water(cx, cz) {
        return;
    }
    let rot = ((h >> 8) & 3) as u8;
    let base_y = editor.get_absolute_y(cx, 1, cz);
    place_structure(editor, schem, cx, cz, base_y, rot, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helicopter_asset_parses() {
        let schem = helicopter().expect("helicopter parses");
        assert!(schem.voxels.len() >= 100);
        assert!(schem.max_extent < 16);
    }
}
