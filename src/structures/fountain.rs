//! Bundled fountains at OSM amenity=fountain; fountain 4 (large) only for big polygons.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, structure_base, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static BYTES: [&[u8]; 4] = [
    include_bytes!("../../assets/structures/fountain1.schem"),
    include_bytes!("../../assets/structures/fountain2.schem"),
    include_bytes!("../../assets/structures/fountain3.schem"),
    include_bytes!("../../assets/structures/fountain4.schem"),
];

/// Footprint (cells) at/above which a fountain is "large" and gets fountain 4.
const LARGE_FOUNTAIN_CELLS: usize = 300;

fn variants() -> &'static [Option<StructureSchematic>; 4] {
    static CELL: OnceLock<[Option<StructureSchematic>; 4]> = OnceLock::new();
    CELL.get_or_init(|| {
        std::array::from_fn(|i| match load_structure(BYTES[i]) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("fountain{} load failed: {e}", i + 1);
                None
            }
        })
    })
}

/// Stamp a fountain at (x, z); `area_cells` is footprint size (0 for a node), large areas get fountain 4 else a random small one (1-3).
pub fn place(editor: &mut WorldEditor, x: i32, z: i32, area_cells: usize) {
    if !editor.place_prop(super::Prop::Fountain) {
        return;
    }
    let variants = variants();
    let h = coord_hash(x, z);
    let pick = if area_cells >= LARGE_FOUNTAIN_CELLS {
        3
    } else {
        (h >> 4) as usize % 3
    };
    // Fall back to any loaded small fountain if the chosen one failed to parse.
    let schem = match variants[pick].as_ref() {
        Some(s) => s,
        None => match variants[..3].iter().flatten().next() {
            Some(s) => s,
            None => return,
        },
    };
    // place_structure would refuse it, after the ground below had been cut.
    if schem.max_extent > crate::tile::TILE_EDITOR_HALO {
        return;
    }
    let rot = ((h >> 8) & 3) as u8;
    // On a slope the basin sits on the lowest ground under it, and the ground pass cuts
    // the rest of its footprint down to that level: from the middle it would hang in the
    // air on the downhill side and sink into the hill on the other. On a steep one it is
    // cut at most MAX_CUT into the hill and stands on a footing on the downhill side.
    let columns = structure_base(schem, x, z, rot, i32::MAX);
    let levels = columns
        .iter()
        .map(|&(cx, cz)| editor.get_ground_level(cx, cz));
    let (Some(low), Some(high)) = (levels.clone().min(), levels.max()) else {
        return;
    };
    let base = low.max(high - MAX_CUT);
    for &(cx, cz) in &columns {
        // A road or another pad already set this column's height; it keeps it.
        if editor.surface_override(cx, cz).is_some() {
            continue;
        }
        let ground = editor.get_ground_level(cx, cz);
        for y in base + 1..=ground {
            editor.set_block_absolute(crate::block_definitions::AIR, cx, y, cz, None, Some(&[]));
        }
        for y in ground + 1..=base {
            editor.set_block_absolute(FOOTING, cx, y, cz, None, Some(&[]));
        }
        editor.register_road_surface_y(cx, cz, base);
    }
    place_structure(editor, schem, x, z, base + 1, rot, None);
}

/// How far a fountain is cut into a hill at most.
const MAX_CUT: i32 = 3;
/// What a fountain stands on where the hill falls away further than that.
const FOOTING: crate::block_definitions::Block = crate::block_definitions::STONE_BRICKS;

#[cfg(test)]
mod tests {
    use super::*;

    /// On a hillside the fountain stands on the lowest ground under it, and the rest of its
    /// footprint is cut down to that level instead of burying the uphill side.
    #[test]
    fn a_fountain_on_a_slope_sits_on_its_lowest_ground() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;
        use crate::ground::Ground;
        use std::sync::Arc;

        // Ground rising one block every two to the east.
        let heights: Vec<Vec<f32>> = (0..64)
            .map(|_| (0..64).map(|x| x as f32 / 2.0).collect())
            .collect();
        let bbox = XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap();
        let llbbox = LLBBox::new(47.0, 8.0, 47.001, 8.001).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &bbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_elevation_test(heights, 64, 64)));
        editor.set_ground_origin(0, 0);
        let (x, z) = (32, 32);
        let before: Vec<i32> = (0..64).map(|cx| editor.get_ground_level(cx, z)).collect();
        place(&mut editor, x, z, 0);

        let h = coord_hash(x, z);
        let schem = variants()[(h >> 4) as usize % 3].as_ref().unwrap();
        let columns = structure_base(schem, x, z, ((h >> 8) & 3) as u8, i32::MAX);
        let low = columns
            .iter()
            .map(|&(cx, _)| before[cx as usize])
            .min()
            .unwrap();
        assert!(
            columns.iter().any(|&(cx, _)| before[cx as usize] > low + 1),
            "the footprint spans the slope"
        );
        for &(cx, cz) in &columns {
            assert_eq!(
                editor.get_ground_level(cx, cz),
                low,
                "levelled at {cx} {cz}"
            );
        }
        let lowest_block = columns
            .iter()
            .filter_map(|&(cx, cz)| {
                (low - 2..low + 20).find(|&y| editor.block_exists_absolute(cx, y, cz))
            })
            .min();
        assert_eq!(
            lowest_block,
            Some(low + 1),
            "the basin rests on the lowest ground"
        );
    }

    /// On a steep hillside the basin is cut no more than MAX_CUT into the hill, and the
    /// downhill side stands on a footing instead of a deep pit being dug.
    #[test]
    fn a_fountain_on_a_steep_slope_is_cut_in_a_little_and_footed() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;
        use crate::ground::Ground;
        use std::sync::Arc;

        // Ground rising two blocks every block to the east.
        let heights: Vec<Vec<f32>> = (0..64)
            .map(|_| (0..64).map(|x| x as f32 * 2.0).collect())
            .collect();
        let bbox = XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap();
        let llbbox = LLBBox::new(47.0, 8.0, 47.001, 8.001).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &bbox, llbbox);
        editor.set_ground(Arc::new(Ground::new_elevation_test(heights, 64, 64)));
        editor.set_ground_origin(0, 0);
        let (x, z) = (32, 32);
        let before: Vec<i32> = (0..64).map(|cx| editor.get_ground_level(cx, z)).collect();
        place(&mut editor, x, z, 0);

        let h = coord_hash(x, z);
        let schem = variants()[(h >> 4) as usize % 3].as_ref().unwrap();
        let columns = structure_base(schem, x, z, ((h >> 8) & 3) as u8, i32::MAX);
        let ground = |cx: i32| before[cx as usize];
        let high = columns.iter().map(|&(cx, _)| ground(cx)).max().unwrap();
        let low = columns.iter().map(|&(cx, _)| ground(cx)).min().unwrap();
        assert!(high - low > MAX_CUT, "the footprint spans a steep slope");
        let base = high - MAX_CUT;
        for &(cx, cz) in &columns {
            assert_eq!(editor.get_ground_level(cx, cz), base, "level at {cx} {cz}");
            if ground(cx) < base {
                assert_eq!(editor.get_block_absolute(cx, base, cz), Some(FOOTING));
            }
        }
    }

    #[test]
    fn fountain_assets_parse() {
        let v = variants();
        for (i, s) in v.iter().enumerate() {
            let s = s
                .as_ref()
                .unwrap_or_else(|| panic!("fountain{} failed", i + 1));
            assert!(!s.voxels.is_empty(), "fountain{} empty", i + 1);
        }
    }
}
