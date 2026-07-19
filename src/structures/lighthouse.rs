//! Bundled lighthouse, placed at OSM man_made=lighthouse features.

use std::collections::HashSet;
use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::block_definitions::{GRASS, OAK_PLANKS, TALL_GRASS_BOTTOM, TALL_GRASS_TOP};
use crate::land_cover::coord_hash;
use crate::trees::schematic::rotate_xz;
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
    if !editor.place_schematics() {
        return;
    }
    let Some(schem) = lighthouse() else {
        return;
    };
    let h = coord_hash(x, z);
    let rot = (h & 3) as u8;
    let base_y = editor.get_absolute_y(x, 1, z);
    place_water_foundation(editor, schem, x, z, rot);
    place_structure(editor, schem, x, z, base_y, rot, None);
}

/// Replace the water surface under the structure's projected footprint with a deck.
fn place_water_foundation(
    editor: &mut WorldEditor,
    schem: &StructureSchematic,
    base_x: i32,
    base_z: i32,
    rot: u8,
) {
    let (anchor_x, anchor_z) =
        rotate_xz(schem.anchor_x, schem.anchor_z, schem.width, schem.length, rot);
    let footprint: HashSet<(i32, i32)> = schem
        .voxels
        .iter()
        .map(|(vx, _, vz, _)| (*vx, *vz))
        .collect();

    for (vx, vz) in footprint {
        let (rx, rz) = rotate_xz(vx, vz, schem.width, schem.length, rot);
        let x = base_x + rx - anchor_x;
        let z = base_z + rz - anchor_z;
        if editor.is_lc_water(x, z) {
            let water_y = editor.get_water_level(x, z);
            if editor.check_for_block_absolute(
                x,
                water_y + 1,
                z,
                Some(&[GRASS, TALL_GRASS_BOTTOM, TALL_GRASS_TOP]),
                None,
            ) {
                continue;
            }
            editor.set_block_absolute(
                OAK_PLANKS,
                x,
                water_y,
                z,
                None,
                Some(&[]),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighthouse_asset_parses() {
        assert!(!lighthouse().expect("lighthouse parses").voxels.is_empty());
    }
}
