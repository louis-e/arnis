//! Bundled playground structures, stamped into leisure=playground areas.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::block_definitions::SAND;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: [&[u8]; 3] = [
    include_bytes!("../../assets/structures/playground1.schem"),
    include_bytes!("../../assets/structures/playground2.schem"),
    include_bytes!("../../assets/structures/playground3.schem"),
];

/// Minimum playground area (placed cells) before any structure appears.
const MIN_CELLS: usize = 120;
/// Minimum gap between placed playground structures.
const SPACING: i32 = 16;

/// Parse the embedded playgrounds once; entries are `None` if a file fails.
fn variants() -> &'static [Option<StructureSchematic>; 3] {
    static CELL: OnceLock<[Option<StructureSchematic>; 3]> = OnceLock::new();
    CELL.get_or_init(|| {
        std::array::from_fn(|i| match load_structure(BYTES[i]) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("playground{} load failed: {e}", i + 1);
                None
            }
        })
    })
}

/// Stamp random playgrounds into a leisure area, each on a sand pad at a random rotation; anchors sampled from cells for tile-seam determinism.
pub fn scatter_playgrounds(editor: &mut WorldEditor, cells: &[(i32, i32)]) {
    if !editor.place_schematics() {
        return;
    }
    let n = cells.len();
    if n < MIN_CELLS {
        return;
    }
    let variants = variants();
    if variants.iter().all(Option::is_none) {
        return;
    }
    // One structure per ~500 cells, a few at most.
    let target = (n / 500).clamp(1, 4);
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
        let Some(schem) = variants[(h >> 5) as usize % 3].as_ref() else {
            continue;
        };
        let rot = ((h >> 7) & 3) as u8;
        let base_y = editor.get_absolute_y(ax, 1, az);
        place_structure(editor, schem, ax, az, base_y, rot, Some(SAND));
        placed.push((ax, az));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playground_assets_parse() {
        let v = variants();
        for (i, s) in v.iter().enumerate() {
            let s = s
                .as_ref()
                .unwrap_or_else(|| panic!("playground{} failed", i + 1));
            assert!(!s.voxels.is_empty(), "playground{} empty", i + 1);
        }
    }
}
