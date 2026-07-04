//! Bundled wind turbine, placed at OSM power=generator generator:source=wind features.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/windturbine.schem");

fn turbine() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(BYTES) {
        // Anchor on the foundation so the wide rotor stays within the tile halo.
        Ok(s) => Some(s.base_anchored()),
        Err(e) => {
            eprintln!("wind turbine schem load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Stamp the wind turbine at (x, z) with a random rotation.
pub fn place(editor: &mut WorldEditor, x: i32, z: i32) {
    if !editor.place_schematics() {
        return;
    }
    let Some(schem) = turbine() else {
        return;
    };
    let rot = (coord_hash(x, z) & 3) as u8;
    let base_y = editor.get_absolute_y(x, 1, z);
    place_structure(editor, schem, x, z, base_y, rot, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turbine_asset_parses() {
        let t = turbine().expect("embedded turbine should parse");
        assert!(t.voxels.len() > 1000, "too few voxels: {}", t.voxels.len());
    }
}
