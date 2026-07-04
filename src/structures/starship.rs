//! Bundled SpaceX Starship, placed on the Starbase Pad 2 launch mount.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/starship.schem");

/// Inner ring of the Pad 2 orbital launch mount at SpaceX Starbase, Boca Chica.
pub const STARBASE_PAD2_INNER_RING_WAY: u64 = 1486752423;

fn starship() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    // Default tallest-column anchor is the rocket centerline.
    CELL.get_or_init(|| match load_structure(BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("starship load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Stamps the Starship upright at the launch mount ring centroid.
pub fn place_on_launch_mount(editor: &mut WorldEditor, ring: &ProcessedWay) {
    if !editor.place_schematics() {
        return;
    }
    let Some(schem) = starship() else {
        return;
    };
    let (mut sx, mut sz, mut n) = (0i64, 0i64, 0i64);
    for node in &ring.nodes {
        sx += node.x as i64;
        sz += node.z as i64;
        n += 1;
    }
    if n == 0 {
        return;
    }
    let (cx, cz) = ((sx / n) as i32, (sz / n) as i32);
    let base_y = editor.get_absolute_y(cx, 1, cz);
    place_structure(editor, schem, cx, cz, base_y, 0, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starship_asset_parses() {
        let schem = starship().expect("starship parses");
        assert!(schem.voxels.len() >= 2000);
        assert!(schem.max_extent < 64);
    }
}
