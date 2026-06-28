//! Bundled lighthouse, placed at OSM man_made=lighthouse features.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/lighthouse.schem");

fn lighthouse() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("lighthouse load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Stamp a lighthouse centred at (x, z) on the ground, at a random rotation.
pub fn place(editor: &mut WorldEditor, x: i32, z: i32) {
    let Some(schem) = lighthouse() else {
        return;
    };
    let h = coord_hash(x, z);
    let rot = (h & 3) as u8;
    let base_y = editor.get_absolute_y(x, 1, z);
    place_structure(editor, schem, x, z, base_y, rot, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighthouse_asset_parses() {
        assert!(!lighthouse().expect("lighthouse parses").voxels.is_empty());
    }
}
