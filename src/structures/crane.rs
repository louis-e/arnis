//! Bundled tower crane, stamped at large construction sites and mapped `man_made=crane`.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::block_definitions::WATER;
use crate::floodfill_cache::CoordinateBitmap;
use crate::land_cover::coord_hash;
use crate::trees::schematic::rotate_xz;
use crate::world_editor::WorldEditor;

static CRANE_BYTES: &[u8] = include_bytes!("../../assets/structures/crane.schem");

/// Minimum construction-site footprint (placed cells) before a crane appears.
const CRANE_MIN_CELLS: usize = 1500;
/// Chance (percent) that a large-enough site actually gets a crane.
const CRANE_SPAWN_PERCENT: u64 = 60;
/// How far out a mapped crane looks for water to swing its jib over.
const CRANE_WATER_REACH: i32 = 40;
/// Crane types the tower crane model does not fit.
const CRANE_TYPES_SKIPPED: [&str; 4] =
    ["travel_lift", "bridge_crane", "overhead_crane", "jib_crane"];

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
pub fn maybe_place_crane(
    editor: &mut WorldEditor,
    cells: &[(i32, i32)],
    building_footprints: &CoordinateBitmap,
) {
    if !editor.place_schematics() {
        return;
    }
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
    // Nor through a building, which if going up has a crane of its own.
    if editor.is_lc_water(ax, az) || building_footprints.contains(ax, az) {
        return;
    }
    if mapped_crane_on(editor.mapped_cranes(), cells).is_some() {
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

/// Stamp the crane with its mast at (x, z) on a building going up, turned by `roll`.
pub fn place_on_site(editor: &mut WorldEditor, x: i32, z: i32, base_y: i32, roll: u64) {
    if !editor.place_schematics() || editor.is_lc_water(x, z) {
        return;
    }
    let Some(schem) = crane() else {
        return;
    };
    place_structure(editor, schem, x, z, base_y, (roll & 3) as u8, None);
}

/// A mapped crane standing on the site, which then gets no crane of its own.
pub fn mapped_crane_on(cranes: &[(i32, i32)], cells: &[(i32, i32)]) -> Option<(i32, i32)> {
    let (Some(min_x), Some(max_x), Some(min_z), Some(max_z)) = (
        cells.iter().map(|c| c.0).min(),
        cells.iter().map(|c| c.0).max(),
        cells.iter().map(|c| c.1).min(),
        cells.iter().map(|c| c.1).max(),
    ) else {
        return None;
    };
    cranes
        .iter()
        .filter(|&&(x, z)| (min_x..=max_x).contains(&x) && (min_z..=max_z).contains(&z))
        .find(|crane| cells.contains(crane))
        .copied()
}

/// Unit step from the mast toward the jib tip once the model is turned `rot` quarters.
fn jib_direction(schem: &StructureSchematic, rot: u8) -> (i32, i32) {
    let (w, l) = (schem.width, schem.length);
    let (ax, az) = rotate_xz(schem.anchor_x, schem.anchor_z, w, l, rot);
    let (dx, dz) = schem
        .voxels
        .iter()
        .map(|&(vx, _, vz, _)| {
            let (rx, rz) = rotate_xz(vx, vz, w, l, rot);
            (rx - ax, rz - az)
        })
        .max_by_key(|&(dx, dz)| dx * dx + dz * dz)
        .unwrap_or((0, 1));
    if dx.abs() >= dz.abs() {
        (dx.signum(), 0)
    } else {
        (0, dz.signum())
    }
}

/// Quarter turn from a `direction` tag in degrees clockwise from north.
fn quarter_for_bearing(schem: &StructureSchematic, degrees: f64) -> u8 {
    let rad = degrees.to_radians();
    let (want_x, want_z) = (rad.sin(), -rad.cos());
    (0..4u8)
        .max_by(|&a, &b| {
            let dot = |r| {
                let (dx, dz) = jib_direction(schem, r);
                f64::from(dx) * want_x + f64::from(dz) * want_z
            };
            dot(a).total_cmp(&dot(b))
        })
        .unwrap_or(0)
}

/// Stamp the crane at a mapped `man_made=crane`, its jib swung over nearby water when there is some.
pub fn place_mapped(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    tags: &HashMap<String, String>,
    building_footprints: &CoordinateBitmap,
) {
    if !editor.place_schematics() {
        return;
    }
    let kind = tags.get("crane:type").map(String::as_str).unwrap_or("");
    // Cranes inside halls run on the roof beams.
    if CRANE_TYPES_SKIPPED.contains(&kind)
        || tags.get("indoor").map(String::as_str) == Some("yes")
        || building_footprints.contains(x, z)
    {
        return;
    }
    let Some(schem) = crane() else {
        return;
    };
    let wet = |px: i32, pz: i32| {
        editor.is_lc_water(px, pz)
            || editor.check_for_block_absolute(
                px,
                editor.get_absolute_y(px, 0, pz),
                pz,
                Some(&[WATER]),
                None,
            )
    };
    let water_toward = |rot: u8| {
        let (dx, dz) = jib_direction(schem, rot);
        (8..=CRANE_WATER_REACH)
            .step_by(4)
            .filter(|d| wet(x + dx * d, z + dz * d))
            .count()
    };
    let rot = match (0..4u8).max_by_key(|&r| water_toward(r)) {
        Some(r) if water_toward(r) > 0 => r,
        _ => match tags
            .get("direction")
            .and_then(|d| d.trim().parse::<f64>().ok())
        {
            Some(deg) => quarter_for_bearing(schem, deg),
            None => (coord_hash(x, z) & 3) as u8,
        },
    };
    let base_y = editor.get_absolute_y(x, 1, z);
    place_structure(editor, schem, x, z, base_y, rot, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jib_turns_with_the_model_and_bearings_pick_the_matching_quarter() {
        let c = crane().expect("embedded crane should parse");
        let dirs: Vec<(i32, i32)> = (0..4).map(|r| jib_direction(c, r)).collect();
        // Four quarter turns point the jib four different ways.
        for (i, a) in dirs.iter().enumerate() {
            for b in &dirs[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(jib_direction(c, quarter_for_bearing(c, 90.0)), (1, 0));
        assert_eq!(jib_direction(c, quarter_for_bearing(c, 180.0)), (0, 1));
    }

    #[test]
    fn a_site_with_a_mapped_crane_on_it_gets_none_of_its_own() {
        let site: Vec<(i32, i32)> = (0..40).flat_map(|x| (0..40).map(move |z| (x, z))).collect();
        assert_eq!(
            mapped_crane_on(&[(100, 100), (20, 20)], &site),
            Some((20, 20))
        );
        assert_eq!(mapped_crane_on(&[(100, 100), (45, 20)], &site), None);
        assert_eq!(mapped_crane_on(&[], &site), None);
    }

    #[test]
    fn crane_asset_parses_to_voxels() {
        let c = crane().expect("embedded crane should parse");
        assert!(c.voxels.len() > 500, "too few voxels: {}", c.voxels.len());
    }
}
