//! Bundled boat, placed in open water (ocean / large lakes, not narrow rivers).

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/boat.schem");

/// Grid step / min gap between boats; large so big lakes stay sparse.
const SPACING: i32 = 400;
/// Percent of grid slots that get a boat.
const CHANCE: u64 = 45;
/// Safety guard only; real density comes from SPACING/CHANCE.
const MAX_BOATS: usize = 200;

fn boat() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("boat load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Scatter boats over open water in a bbox, one below surface, random rotation; grid on a global lattice so tiles agree.
pub fn scatter_boats(editor: &mut WorldEditor, min_x: i32, min_z: i32, max_x: i32, max_z: i32) {
    if !editor.place_schematics() {
        return;
    }
    let Some(schem) = boat() else {
        return;
    };
    let mut count = 0usize;
    let mut gz = min_z - min_z.rem_euclid(SPACING);
    while gz <= max_z {
        let mut gx = min_x - min_x.rem_euclid(SPACING);
        while gx <= max_x {
            if count >= MAX_BOATS {
                return;
            }
            let h = coord_hash(gx, gz);
            if h % 100 < CHANCE {
                let ax = gx + (h % 7) as i32;
                let az = gz + ((h >> 3) % 7) as i32;
                // water_distance caps at 15, so a water cell still at 0 is past the cap:
                // deep open water, not a river or shore. Slots decide independently (tile-invariant).
                if editor.is_lc_water(ax, az) && editor.water_distance(ax, az) == 0 {
                    let base_y = editor.get_water_level(ax, az) - 1;
                    let rot = ((h >> 5) & 3) as u8;
                    place_structure(editor, schem, ax, az, base_y, rot, None);
                    count += 1;
                }
            }
            gx += SPACING;
        }
        gz += SPACING;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boat_asset_parses() {
        assert!(!boat().expect("boat parses").voxels.is_empty());
    }
}
