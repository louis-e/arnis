//! Bundled excavator, scattered across large construction sites.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static EXCAVATOR_BYTES: &[u8] = include_bytes!("../../assets/structures/excavator.schem");

/// Same "big enough" gate as the crane before any excavator appears.
const EXCAVATOR_MIN_CELLS: usize = 1500;
/// Minimum gap between scattered excavators (also their rough footprint).
const SPACING: i32 = 24;

fn excavator() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(EXCAVATOR_BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("excavator schem load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Scatter excavators across a large site at random rotations; anchors sampled from the stable cell list, so placement is deterministic across tile seams.
pub fn scatter_excavators(editor: &mut WorldEditor, cells: &[(i32, i32)]) {
    if !editor.place_schematics() {
        return;
    }
    let n = cells.len();
    if n < EXCAVATOR_MIN_CELLS {
        return;
    }
    let Some(schem) = excavator() else {
        return;
    };
    // Roughly one excavator per ~2000 cells, capped to a few.
    let target = (n / 2000).clamp(1, 6);
    let mut placed: Vec<(i32, i32)> = Vec::new();
    let max_attempts = target as u32 * 8;
    let mut t: u32 = 0;
    while placed.len() < target && t < max_attempts {
        let h = coord_hash(t as i32 + 1, n as i32);
        t += 1;
        let (ax, az) = cells[(h % n as u64) as usize];
        if editor.is_lc_water(ax, az) {
            continue;
        }
        let too_close = placed
            .iter()
            .any(|&(px, pz)| (px - ax).abs() < SPACING && (pz - az).abs() < SPACING);
        if too_close {
            continue;
        }
        let rot = ((h >> 5) & 3) as u8;
        let base_y = editor.get_absolute_y(ax, 1, az);
        place_structure(editor, schem, ax, az, base_y, rot, None);
        placed.push((ax, az));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excavator_asset_parses_to_voxels() {
        let e = excavator().expect("embedded excavator should parse");
        assert!(e.voxels.len() > 100, "too few voxels: {}", e.voxels.len());
    }
}
