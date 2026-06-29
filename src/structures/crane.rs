//! Bundled construction crane, stamped at large construction sites.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static CRANE_BYTES: &[u8] = include_bytes!("../../assets/structures/crane.schem");

/// Minimum construction-site footprint (placed cells) before a crane appears.
const CRANE_MIN_CELLS: usize = 1500;
/// Chance (percent) that a large-enough site actually gets a crane.
const CRANE_SPAWN_PERCENT: u64 = 60;

/// Parse the embedded crane once; `None` if the asset fails to load.
fn crane() -> Option<&'static StructureSchematic> {
    static CELL: OnceLock<Option<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match load_structure(CRANE_BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("crane schem load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Place one crane at the centroid of a large-enough site; mast on-site, jib overhangs (clipped at edge).
pub fn maybe_place_crane(editor: &mut WorldEditor, cells: &[(i32, i32)]) {
    if cells.len() < CRANE_MIN_CELLS {
        return;
    }
    let Some(schem) = crane() else {
        return;
    };
    // Centroid, then snap to the nearest actual cell so the mast lands on-site.
    let (mut sx, mut sz) = (0i64, 0i64);
    for &(x, z) in cells {
        sx += x as i64;
        sz += z as i64;
    }
    let cx = (sx / cells.len() as i64) as i32;
    let cz = (sz / cells.len() as i64) as i32;
    let (ax, az) = cells
        .iter()
        .copied()
        .min_by_key(|&(x, z)| {
            let (dx, dz) = ((x - cx) as i64, (z - cz) as i64);
            dx * dx + dz * dz
        })
        .unwrap_or((cx, cz));
    if editor.is_lc_water(ax, az) {
        return;
    }
    // Not every site gets one; random facing, seeded by anchor for tile-seam determinism.
    let h = coord_hash(ax, az);
    if h % 100 >= CRANE_SPAWN_PERCENT {
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
    fn crane_asset_parses_to_voxels() {
        let c = crane().expect("embedded crane should parse");
        assert!(c.voxels.len() > 500, "too few voxels: {}", c.voxels.len());
    }
}
