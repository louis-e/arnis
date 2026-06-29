//! Bundled tractor, occasionally placed on farmland fields.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/tractor.schem");

/// Minimum field size (cells) before a tractor may appear.
const MIN_CELLS: usize = 600;
/// Chance (percent) a qualifying field actually gets one (kept rare).
const SPAWN_PERCENT: u64 = 30;

fn tractor() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("tractor load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Rarely drop one tractor on a farmland field at a random rotation; seeded by cells for tile-seam determinism.
pub fn maybe_place_tractor(editor: &mut WorldEditor, cells: &[(i32, i32)]) {
    let n = cells.len();
    if n < MIN_CELLS {
        return;
    }
    let Some(schem) = tractor() else {
        return;
    };
    let h = coord_hash(cells[0].0, cells[0].1 ^ n as i32);
    if h % 100 >= SPAWN_PERCENT {
        return;
    }
    let (ax, az) = cells[(h % n as u64) as usize];
    if editor.is_lc_water(ax, az) {
        return;
    }
    let rot = ((h >> 8) & 3) as u8;
    let base_y = editor.get_absolute_y(ax, 1, az);
    place_structure(editor, schem, ax, az, base_y, rot, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tractor_asset_parses() {
        assert!(!tractor().expect("tractor parses").voxels.is_empty());
    }
}
