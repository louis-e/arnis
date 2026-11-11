//! Bundled lighthouse, placed at OSM man_made=lighthouse features.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, structure_base, StructureSchematic};
use crate::block_definitions::STONE_BRICKS;
use crate::element_processing::waterfront::drive_pile;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/lighthouse.schem");

/// Layers of the model's ground storey. Its floors start up to nine layers above the
/// ground, so these columns are the whole base, and an overhang higher up gets no footing.
const GROUND_STOREY: i32 = 10;

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
    if !editor.place_prop(super::Prop::Lighthouse) {
        return;
    }
    let Some(schem) = lighthouse() else {
        return;
    };
    let h = coord_hash(x, z);
    let rot = (h & 3) as u8;
    let base_y = editor.get_absolute_y(x, 1, z);
    if !place_structure(editor, schem, x, z, base_y, rot, None) {
        return;
    }
    // The model's floor is the ground it stands on. Out on a breakwater or in
    // the water there is none, so the tower gets a stone footing: up from
    // ground falling away under it, and down through the water to the bed.
    for (cx, cz) in structure_base(schem, x, z, rot, GROUND_STOREY) {
        drive_pile(editor, cx, cz, base_y - 1, STONE_BRICKS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighthouse_asset_parses() {
        assert!(!lighthouse().expect("lighthouse parses").voxels.is_empty());
    }

    #[test]
    fn a_lighthouse_in_the_water_stands_on_a_stone_footing() {
        use crate::block_definitions::WATER;
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;
        let bbox = XZBBox::rect_from_min_max(-48, -48, 48, 48).unwrap();
        let llbbox = LLBBox::new(59.96, 30.21, 59.97, 30.22).unwrap();
        let mut editor =
            WorldEditor::new(std::path::PathBuf::from("/dev/null/unused"), &bbox, llbbox);
        // Flat ground at Y 0 without terrain: water from the surface three blocks down.
        for x in -40..=40 {
            for z in -40..=40 {
                for y in -3..=0 {
                    editor.set_block_absolute(WATER, x, y, z, None, None);
                }
            }
        }
        place(&mut editor, 0, 0);
        let columns = structure_base(
            lighthouse().unwrap(),
            0,
            0,
            (coord_hash(0, 0) & 3) as u8,
            GROUND_STOREY,
        );
        assert!(!columns.is_empty());
        for (x, z) in columns {
            for y in -3..=0 {
                assert!(
                    editor.check_for_block_absolute(x, y, z, Some(&[STONE_BRICKS]), None),
                    "water left at {x} {y} {z}"
                );
            }
            // The footing stands under the model, not beside it.
            assert!(
                (1..=GROUND_STOREY).any(|y| editor
                    .get_block_absolute(x, y, z)
                    .is_some_and(|b| b != crate::block_definitions::AIR)),
                "no model over the footing at {x} {z}"
            );
        }
    }
}
