//! Greenhouses: Venlo glass roofs, plant rows inside, and halls on horticulture land without any.

use crate::block_definitions::*;
use crate::element_processing::area_layout::{cell_hash, AreaMask};
use crate::element_processing::landuse::ground_is_protected;
use crate::floodfill_cache::{in_hole, BuildingFootprintBitmap, Holes, RoadMaskBitmap};
use crate::land_cover::coord_hash;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// Blocks from one gutter to the next across a Venlo roof.
const SPAN: i32 = 4;

pub fn is_greenhouse(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("building")
            .or_else(|| tags.get("building:part"))
            .map(String::as_str),
        Some("greenhouse" | "glasshouse")
    )
}

/// A greenhouse whose roof the map leaves open, which gets glass ridges.
pub fn wants_venlo_roof(tags: &HashMap<String, String>) -> bool {
    is_greenhouse(tags) && !tags.contains_key("roof:shape")
}

/// Position across the ridges; world-anchored, so neighbouring houses and tiles line up.
fn across(x: i32, z: i32, along_x: bool) -> i32 {
    (if along_x { z } else { x }).rem_euclid(SPAN)
}

/// Ridges of glass along `along_x` over gutters, with the roof line at `roof_y`.
pub fn venlo_roof(editor: &mut WorldEditor, area: &[(i32, i32)], along_x: bool, roof_y: i32) {
    let force: &[Block] = &[];
    for &(x, z) in area {
        match across(x, z, along_x) {
            0 => editor.set_block_absolute(SMOOTH_STONE_SLAB, x, roof_y, z, None, Some(force)),
            2 => {
                editor.set_block_absolute(AIR, x, roof_y, z, None, Some(force));
                editor.set_block_absolute(GLASS, x, roof_y + 1, z, None, Some(force));
            }
            _ => editor.set_block_absolute(GLASS, x, roof_y, z, None, Some(force)),
        }
    }
}

/// What a greenhouse grows, in rows along its ridges.
#[derive(Clone, Copy)]
enum Crop {
    Tomatoes,
    Flowers,
    Seedlings,
}

impl Crop {
    fn of(seed: u64) -> Self {
        match seed % 3 {
            0 => Crop::Tomatoes,
            1 => Crop::Flowers,
            _ => Crop::Seedlings,
        }
    }

    /// Plants of one column of a row on a bed of soil, from the floor up.
    fn plant(self, editor: &mut WorldEditor, x: i32, floor_y: i32, z: i32, row: u64) {
        let soil = match self {
            Crop::Tomatoes => DIRT,
            Crop::Flowers | Crop::Seedlings => GRASS_BLOCK,
        };
        editor.set_block_absolute(soil, x, floor_y, z, None, Some(&[]));
        let put = |editor: &mut WorldEditor, y: i32, b: Block| {
            if !editor.block_exists_absolute(x, y, z) {
                editor.set_block_absolute(b, x, y, z, None, None);
            }
        };
        match self {
            Crop::Tomatoes => {
                let accent = coord_hash(x, z ^ 0x70).is_multiple_of(10);
                put(editor, floor_y + 1, AZALEA_LEAVES);
                let top = if accent {
                    FLOWERING_AZALEA_LEAVES
                } else {
                    AZALEA_LEAVES
                };
                put(editor, floor_y + 2, top);
            }
            Crop::Flowers => put(editor, floor_y + 1, super::cemetery::flower(row)),
            Crop::Seedlings => put(
                editor,
                floor_y + 1,
                if row.is_multiple_of(2) { FERN } else { GRASS },
            ),
        }
    }
}

/// Rows of plants on the floor of a greenhouse, clear of its walls.
pub fn plant_rows(
    editor: &mut WorldEditor,
    area: &[(i32, i32)],
    along_x: bool,
    floor_y: i32,
    seed: u64,
) {
    let mask = AreaMask::new(area, |_, _| true);
    let crop = Crop::of(seed);
    for &(x, z) in area {
        if !editor.owns(x, z) || across(x, z, along_x) % 2 == 1 {
            continue;
        }
        let inside = (-2..=2).all(|dx| (-2..=2).all(|dz| mask.contains(x + dx, z + dz)));
        if inside {
            let row = (if along_x { z } else { x }).div_euclid(2) as u64 ^ seed;
            crop.plant(editor, x, floor_y, z, row);
        }
    }
}

/// Glasshouse halls, block by block between lanes, on horticulture land the map leaves empty.
pub fn horticulture_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    fill: &[(i32, i32)],
    road_mask: &RoadMaskBitmap,
    footprints: &BuildingFootprintBitmap,
    holes: Option<&Holes>,
) {
    let mut built = false;
    let mask = AreaMask::new(fill, |x, z| {
        built |= footprints.contains(x, z);
        !in_hole(holes, x, z)
    });
    let s = editor.scale().clamp(0.25, 4.0);
    let (hall_x, hall_z) = ((64.0 * s) as i32, (40.0 * s) as i32);
    let lane = ((6.0 * s) as i32).max(3);
    let height = ((4.0 * s).round() as i32).max(3);
    let (px, pz) = (hall_x + lane, hall_z + lane);
    // Lanes are world-anchored, so a field split over several ways still lines up.
    let cell = |x: i32, z: i32| -> Option<(i32, i32)> {
        let (ix, iz) = (x.div_euclid(px), z.div_euclid(pz));
        let (lx, lz) = (x - ix * px, z - iz * pz);
        (lx >= lane && lz >= lane).then_some((ix, iz))
    };
    let hall_at = |x: i32, z: i32| {
        mask.contains(x, z)
            && cell(x, z).is_some()
            && !road_mask.contains(x, z)
            && !footprints.contains(x, z)
    };
    for &(x, z) in fill {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let hall = !built && hall_at(x, z);
        let ground = if built {
            GRASS_BLOCK
        } else if hall {
            GRAVEL
        } else {
            COARSE_DIRT
        };
        editor.set_block(ground, x, 0, z, None, None);
        if !hall || !editor.owns(x, z) || editor.is_lc_water(x, z) {
            continue;
        }
        let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|(dx, dz)| !hall_at(x + dx, z + dz));
        let base = editor.get_absolute_y(x, 0, z);
        let force: &[Block] = &[];
        if edge {
            for y in 1..=height {
                editor.set_block_absolute(WHITE_STAINED_GLASS, x, base + y, z, None, Some(force));
            }
        }
        // Ridges along x, as the halls run.
        let roof_y = base + height + 1;
        match z.rem_euclid(SPAN) {
            0 => editor.set_block_absolute(SMOOTH_STONE_SLAB, x, roof_y, z, None, Some(force)),
            2 => {
                editor.set_block_absolute(WHITE_STAINED_GLASS, x, roof_y + 1, z, None, Some(force))
            }
            _ => editor.set_block_absolute(WHITE_STAINED_GLASS, x, roof_y, z, None, Some(force)),
        }
        if !edge && z.rem_euclid(2) == 0 {
            let (ix, iz) = cell(x, z).unwrap_or((0, 0));
            let crop = Crop::of(cell_hash(way.id, ix as i64, iz as i64));
            crop.plant(editor, x, base, z, (z.div_euclid(2) as u64) ^ way.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::*;

    #[test]
    fn a_venlo_roof_ridges_every_four_blocks() {
        let xzbbox = XZBBox::rect_from_xz_lengths(20.0, 20.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let area: Vec<(i32, i32)> = (2..14).flat_map(|x| (2..14).map(move |z| (x, z))).collect();
        venlo_roof(&mut editor, &area, true, 10);
        for z in 2..14 {
            let (low, high) = (
                editor.get_block_absolute(6, 10, z),
                editor.get_block_absolute(6, 11, z),
            );
            match z % 4 {
                0 => assert_eq!(low, Some(SMOOTH_STONE_SLAB), "gutter at z={z}"),
                2 => assert_eq!((low.is_none(), high), (true, Some(GLASS)), "ridge at z={z}"),
                _ => assert_eq!(low, Some(GLASS), "pane at z={z}"),
            }
        }
    }

    #[test]
    fn plants_grow_on_soil_not_on_the_floor() {
        let xzbbox = XZBBox::rect_from_xz_lengths(20.0, 20.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let area: Vec<(i32, i32)> = (2..16).flat_map(|x| (2..16).map(move |z| (x, z))).collect();
        for &(x, z) in &area {
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, 0, z, None, None);
        }
        plant_rows(&mut editor, &area, true, 0, 3);
        let (mut planted, mut on_soil) = (0, 0);
        for &(x, z) in &area {
            if editor.block_exists_absolute(x, 1, z) {
                planted += 1;
                on_soil +=
                    editor.check_for_block_absolute(x, 0, z, Some(&[DIRT, GRASS_BLOCK]), None)
                        as usize;
            }
        }
        assert!(planted > 20, "{planted} planted");
        assert_eq!(on_soil, planted, "every plant on soil");
    }

    #[test]
    fn bare_horticulture_land_gets_glasshouses_but_mapped_ones_stand_alone() {
        let xzbbox = XZBBox::rect_from_xz_lengths(160.0, 110.0).unwrap();
        let way = rect_way(
            31,
            2,
            2,
            157,
            107,
            &[("landuse", "greenhouse_horticulture")],
        );
        let fill = crate::floodfill::flood_fill_area(
            &way.nodes.iter().map(|n| (n.x, n.z)).collect::<Vec<_>>(),
            None,
        );
        let glass = |footprints: &BuildingFootprintBitmap| {
            let mut editor = test_editor(&xzbbox);
            horticulture_area(
                &mut editor,
                &way,
                &fill,
                &RoadMaskBitmap::new_empty(),
                footprints,
                None,
            );
            (1..6)
                .map(|y| count_blocks(&editor, 3..157, 3..107, y, &[WHITE_STAINED_GLASS]))
                .sum::<usize>()
        };
        assert!(glass(&BuildingFootprintBitmap::new_empty()) > 500);
        assert_eq!(glass(&bitmap_with_rect(&xzbbox, 50, 50, 60, 60)), 0);
    }
}
