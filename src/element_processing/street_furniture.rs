//! Bicycle stands, shelters and picnic tables.

use crate::block_definitions::*;
use crate::element_processing::connected_blocks::{connected_iron_bars, place_connected};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// Radius searched for the roads a node lines up with.
const ROAD_FRAME_RADIUS: i32 = 8;
/// Stands a bicycle parking node gets without a `capacity` tag.
const DEFAULT_STANDS: i32 = 3;
const MAX_STANDS_PER_NODE: i32 = 8;
/// Rows of stands in a parking area are this many blocks apart, aisle included.
const STAND_ROW_PITCH: i32 = 4;

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

/// Axis the roads around (x, z) run along and the unit step toward them, or a seeded guess.
fn road_frame(x: i32, z: i32, road_mask: &RoadMaskBitmap, seed: u64) -> (bool, (i32, i32)) {
    let r = ROAD_FRAME_RADIUS;
    let (mut n, mut sx, mut sz, mut sxx, mut szz) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for dz in -r..=r {
        for dx in -r..=r {
            if dx * dx + dz * dz <= r * r && road_mask.contains(x + dx, z + dz) {
                let (fx, fz) = (dx as f64, dz as f64);
                n += 1.0;
                sx += fx;
                sz += fz;
                sxx += fx * fx;
                szz += fz * fz;
            }
        }
    }
    if n < 3.0 {
        let along_x = seed.is_multiple_of(2);
        let side = if seed % 4 < 2 { 1 } else { -1 };
        return (along_x, if along_x { (0, side) } else { (side, 0) });
    }
    let (mx, mz) = (sx / n, sz / n);
    let along_x = sxx / n - mx * mx >= szz / n - mz * mz;
    let sign = |v: f64| if v < 0.0 { -1 } else { 1 };
    (
        along_x,
        if along_x {
            (0, sign(mz))
        } else {
            (sign(mx), 0)
        },
    )
}

/// Iron bars spanning the cell along x or z, the side view of a hoop stand.
fn hoop(spans_x: bool) -> BlockWithProperties {
    connected_iron_bars(!spans_x, !spans_x, spans_x, spans_x)
}

/// A row of hoop stands, two bikes each, lined up with the street.
pub fn generate_bicycle_stands(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &RoadMaskBitmap,
) {
    let stands = tag(&node.tags, "capacity")
        .and_then(|c| c.trim().parse::<i32>().ok())
        .map_or(DEFAULT_STANDS, |c| (c + 1) / 2)
        .clamp(1, MAX_STANDS_PER_NODE);
    let (along_x, _) = road_frame(node.x, node.z, road_mask, node.id);
    // Hoops stand across the row so the bikes park between them.
    let stand = hoop(!along_x);
    for k in 0..stands {
        let offset = 2 * k - (stands - 1);
        let (x, z) = if along_x {
            (node.x + offset, node.z)
        } else {
            (node.x, node.z + offset)
        };
        let y = editor.get_absolute_y(x, 1, z);
        editor.set_block_with_properties_absolute(stand.clone(), x, y, z, None, None);
    }
}

/// Rows of stands across a parking area; a roof only when it is covered.
pub fn generate_bicycle_parking_area(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    area: &[(i32, i32)],
) {
    let tags = element.tags();
    if let Some(floor) = tag(tags, "surface")
        .and_then(crate::element_processing::surfaces::get_blocks_for_surface)
        .map(|blocks| blocks[0])
    {
        for &(x, z) in area {
            editor.set_block(floor, x, 0, z, None, None);
        }
    }

    let (Some(min_x), Some(max_x), Some(min_z), Some(max_z)) = (
        area.iter().map(|c| c.0).min(),
        area.iter().map(|c| c.0).max(),
        area.iter().map(|c| c.1).min(),
        area.iter().map(|c| c.1).max(),
    ) else {
        return;
    };
    // Rows follow the long side.
    let along_x = max_x - min_x >= max_z - min_z;
    let (across_min, across_max) = if along_x {
        (min_z, max_z)
    } else {
        (min_x, max_x)
    };
    let span = across_max - across_min + 1;
    let in_row = |across: i32| {
        if span < STAND_ROW_PITCH + 1 {
            across == across_min + span / 2
        } else {
            (across - across_min) % STAND_ROW_PITCH == STAND_ROW_PITCH / 2
        }
    };
    let stand = hoop(!along_x);
    let inside: std::collections::HashSet<(i32, i32)> = area.iter().copied().collect();
    for &(x, z) in area {
        let (along, across) = if along_x { (x, z) } else { (z, x) };
        if along % 2 != 0 || !in_row(across) {
            continue;
        }
        // Keep the hoops off the outline so the bikes stay inside.
        let (ax, az) = if along_x { (1, 0) } else { (0, 1) };
        if !inside.contains(&(x + ax, z + az)) || !inside.contains(&(x - ax, z - az)) {
            continue;
        }
        let y = editor.get_absolute_y(x, 1, z);
        editor.set_block_with_properties_absolute(stand.clone(), x, y, z, None, None);
    }

    let covered = tag(tags, "covered") == Some("yes")
        || matches!(tag(tags, "bicycle_parking"), Some("shed" | "building"));
    if covered {
        for node in element.nodes() {
            for y in 1..=3 {
                editor.set_block(SPRUCE_FENCE, node.x, y, node.z, None, None);
            }
        }
        for &(x, z) in area {
            editor.set_block(SMOOTH_STONE_SLAB, x, 4, z, None, None);
        }
    }
}

/// The kinds of point shelters drawn differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShelterKind {
    /// Glass bus shelter with a bench.
    Transit,
    /// Open pavilion on four posts under a hipped roof.
    Pavilion { table: bool },
    /// Hut open at the front.
    LeanTo,
}

fn shelter_kind(tags: &HashMap<String, String>) -> Option<ShelterKind> {
    match tag(tags, "shelter_type") {
        Some("public_transport") => Some(ShelterKind::Transit),
        Some("rock_shelter") => None,
        Some("lean_to" | "basic_hut" | "weather_shelter" | "field_shelter" | "changing_rooms") => {
            Some(ShelterKind::LeanTo)
        }
        Some("gazebo" | "sun_shelter" | "pavilion") => Some(ShelterKind::Pavilion { table: false }),
        _ if tag(tags, "bus") == Some("yes") || tag(tags, "public_transport").is_some() => {
            Some(ShelterKind::Transit)
        }
        _ => Some(ShelterKind::Pavilion { table: true }),
    }
}

/// A shelter mapped as a point, built to its `shelter_type`.
pub fn generate_shelter_node(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    road_mask: &RoadMaskBitmap,
) {
    let Some(kind) = shelter_kind(&node.tags) else {
        return;
    };
    let (along_x, toward) = road_frame(node.x, node.z, road_mask, node.id);
    let base = editor.get_absolute_y(node.x, 0, node.z);
    match kind {
        ShelterKind::Transit => transit_shelter(editor, node.x, node.z, base, along_x, toward),
        ShelterKind::Pavilion { table } => {
            pavilion(editor, node.x, node.z, base);
            if table {
                picnic_table(editor, node.x, node.z, base, along_x);
            }
        }
        ShelterKind::LeanTo => lean_to(editor, node.x, node.z, base, along_x, toward),
    }
}

/// Cell `a` along the street and `d` back from it.
fn frame_cell(x: i32, z: i32, along_x: bool, toward: (i32, i32), a: i32, d: i32) -> (i32, i32) {
    let (ax, az) = if along_x { (1, 0) } else { (0, 1) };
    (x + ax * a - toward.0 * d, z + az * a - toward.1 * d)
}

/// Fills a column from its own ground up to `top`, so posts reach down a slope.
fn post(editor: &mut WorldEditor, block: Block, x: i32, z: i32, top: i32) {
    let ground = editor.get_absolute_y(x, 0, z);
    for y in (ground + 1)..=top {
        editor.set_block_absolute(block, x, y, z, None, None);
    }
}

/// Five wide, two deep: glass back and sides, a bench, a flat roof.
fn transit_shelter(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    base: i32,
    along_x: bool,
    toward: (i32, i32),
) {
    let cell = |a: i32, d: i32| frame_cell(x, z, along_x, toward, a, d);
    let glass: Vec<(i32, i32)> = (-2..=2)
        .map(|a| cell(a, 1))
        .chain([cell(-2, 0), cell(2, 0)])
        .collect();
    for &(gx, gz) in &glass {
        let ground = editor.get_absolute_y(gx, 0, gz);
        for y in (ground + 1)..=(base + 2) {
            place_connected(editor, GLASS_PANE, gx, y, gz);
        }
    }
    for a in -1..=1 {
        let (bx, bz) = cell(a, 0);
        editor.set_block_absolute(OAK_SLAB_TOP, bx, base + 1, bz, None, None);
    }
    for a in -2..=2 {
        for d in 0..=1 {
            let (rx, rz) = cell(a, d);
            editor.set_block_absolute(SMOOTH_STONE_SLAB, rx, base + 3, rz, None, None);
        }
    }
}

/// Stair facing toward the centre for a hip roof ring cell at (dx, dz), radius r.
fn hip_stair(dx: i32, dz: i32, r: i32) -> BlockWithProperties {
    use StairFacing::*;
    use StairShape::*;
    let (facing, shape) = match (
        dx.signum() * (dx.abs() == r) as i32,
        dz.signum() * (dz.abs() == r) as i32,
    ) {
        (1, 1) => (North, OuterLeft),
        (-1, 1) => (North, OuterRight),
        (1, -1) => (South, OuterRight),
        (-1, -1) => (South, OuterLeft),
        (0, 1) => (North, Straight),
        (0, -1) => (South, Straight),
        (1, 0) => (West, Straight),
        _ => (East, Straight),
    };
    create_stair_with_properties(SPRUCE_STAIRS, facing, shape)
}

/// Five by five, posts at the corners, hipped roof of two stair rings and a capped top.
fn pavilion(editor: &mut WorldEditor, x: i32, z: i32, base: i32) {
    for (dx, dz) in [(-2, -2), (2, -2), (-2, 2), (2, 2)] {
        post(editor, SPRUCE_FENCE, x + dx, z + dz, base + 3);
    }
    for r in 1..=2 {
        let y = base + 6 - r;
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dz.abs()) == r {
                    editor.set_block_with_properties_absolute(
                        hip_stair(dx, dz, r),
                        x + dx,
                        y,
                        z + dz,
                        None,
                        None,
                    );
                }
            }
        }
    }
    editor.set_block_absolute(SPRUCE_PLANKS, x, base + 5, z, None, None);
    editor.set_block_absolute(SPRUCE_SLAB, x, base + 6, z, None, None);
}

/// Three long table with a bench either side, centred on (x, z).
pub fn picnic_table(editor: &mut WorldEditor, x: i32, z: i32, base: i32, along_x: bool) {
    for a in -1..=1 {
        for side in -1..=1 {
            let (cx, cz) = if along_x {
                (x + a, z + side)
            } else {
                (x + side, z + a)
            };
            let block = if side == 0 { OAK_SLAB_TOP } else { OAK_SLAB };
            editor.set_block_absolute(block, cx, base + 1, cz, None, None);
        }
    }
}

/// Five wide, three deep: plank back and sides, open front, a bench along the back.
fn lean_to(editor: &mut WorldEditor, x: i32, z: i32, base: i32, along_x: bool, toward: (i32, i32)) {
    let cell = |a: i32, d: i32| frame_cell(x, z, along_x, toward, a, d);
    for a in -2..=2 {
        for d in 0..=2 {
            let (cx, cz) = cell(a, d);
            if a.abs() == 2 || d == 2 {
                post(editor, SPRUCE_PLANKS, cx, cz, base + 2);
            }
        }
    }
    for a in -1..=1 {
        let (bx, bz) = cell(a, 1);
        editor.set_block_absolute(SPRUCE_SLAB, bx, base + 1, bz, None, None);
    }
    for a in -2..=2 {
        for d in -1..=2 {
            let (rx, rz) = cell(a, d);
            editor.set_block_absolute(SPRUCE_SLAB, rx, base + 3, rz, None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{tag_map, test_editor};
    use fastnbt::Value;

    fn node(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: tag_map(tags),
            x,
            z,
        }
    }

    fn road_along_x(xzbbox: &XZBBox, z: i32) -> RoadMaskBitmap {
        let mut mask = RoadMaskBitmap::new(xzbbox);
        for x in 0..40 {
            for dz in 0..3 {
                mask.set(x, z + dz);
            }
        }
        mask
    }

    #[test]
    fn the_road_frame_follows_the_street_and_points_at_it() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mask = road_along_x(&xzbbox, 24);
        assert_eq!(road_frame(20, 20, &mask, 0), (true, (0, 1)));
        let mask = road_along_x(&xzbbox, 14);
        assert_eq!(road_frame(20, 20, &mask, 0), (true, (0, -1)));
    }

    #[test]
    fn stands_line_up_with_the_street_and_count_from_capacity() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let mask = road_along_x(&xzbbox, 24);
        generate_bicycle_stands(&mut editor, &node(1, 20, 20, &[("capacity", "10")]), &mask);
        let y = editor.get_absolute_y(20, 1, 20);
        let hoops: Vec<i32> = (10..=30)
            .filter(|&x| editor.get_block_absolute(x, y, 20) == Some(IRON_BARS))
            .collect();
        assert_eq!(hoops, vec![16, 18, 20, 22, 24]);
        // Each hoop spans the cell across the row.
        let Some(Value::Compound(p)) = editor.block_properties_absolute(20, y, 20) else {
            panic!("hoop without state");
        };
        assert_eq!(p.get("north"), Some(&Value::String("true".into())));
        assert_eq!(p.get("east"), Some(&Value::String("false".into())));
    }

    #[test]
    fn a_bus_shelter_turns_its_open_side_to_the_road() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let mask = road_along_x(&xzbbox, 24);
        let shelter = node(
            2,
            20,
            20,
            &[("amenity", "shelter"), ("shelter_type", "public_transport")],
        );
        generate_shelter_node(&mut editor, &shelter, &mask);
        let y = editor.get_absolute_y(20, 1, 20);
        // Glass behind, away from the road; the bench at the node; open toward the road.
        assert_eq!(editor.get_block_absolute(20, y, 19), Some(GLASS_PANE));
        assert_eq!(editor.get_block_absolute(20, y, 20), Some(OAK_SLAB_TOP));
        assert_eq!(editor.get_block_absolute(20, y, 21), None);
        assert_eq!(
            editor.get_block_absolute(20, y + 2, 20),
            Some(SMOOTH_STONE_SLAB)
        );
    }

    #[test]
    fn parking_areas_get_rows_of_stands_and_a_roof_only_when_covered() {
        use crate::element_processing::building_test_support::rect_way;
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let area: Vec<(i32, i32)> = (10..=20)
            .flat_map(|x| (10..=13).map(move |z| (x, z)))
            .collect();
        let draw = |tags: &[(&str, &str)]| {
            let mut editor = test_editor(&xzbbox);
            let way = ProcessedElement::Way(rect_way(1, 10, 10, 20, 13, tags));
            generate_bicycle_parking_area(&mut editor, &way, &area);
            let y = editor.get_absolute_y(12, 1, 12);
            let stands = area
                .iter()
                .filter(|&&(x, z)| editor.get_block_absolute(x, y, z) == Some(IRON_BARS))
                .count();
            let roofed = editor.get_block_absolute(15, y + 3, 12) == Some(SMOOTH_STONE_SLAB);
            (stands, roofed)
        };
        let (stands, roofed) = draw(&[("amenity", "bicycle_parking")]);
        assert!(stands >= 4, "only {stands} stands");
        assert!(!roofed);
        assert!(draw(&[("amenity", "bicycle_parking"), ("covered", "yes")]).1);
    }

    #[test]
    fn rock_shelters_draw_nothing_and_the_rest_get_a_roof() {
        assert_eq!(
            shelter_kind(&tag_map(&[("shelter_type", "rock_shelter")])),
            None
        );
        assert_eq!(
            shelter_kind(&tag_map(&[("shelter_type", "picnic_shelter")])),
            Some(ShelterKind::Pavilion { table: true })
        );
        assert_eq!(
            shelter_kind(&tag_map(&[("shelter_type", "weather_shelter")])),
            Some(ShelterKind::LeanTo)
        );
    }
}
