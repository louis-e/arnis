//! Bundled cars, parked on amenity=parking lots.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static FEDEX: &[u8] = include_bytes!("../../assets/structures/car_fedex.schem");
static HOTROD_WHITE: &[u8] = include_bytes!("../../assets/structures/car_hotrod_white.schem");
static HOTROD_BLUE: &[u8] = include_bytes!("../../assets/structures/car_hotrod_blue.schem");
static POLICE: &[u8] = include_bytes!("../../assets/structures/car_police.schem");
static UHAUL: &[u8] = include_bytes!("../../assets/structures/car_uhaul.schem");
static WORKVAN: &[u8] = include_bytes!("../../assets/structures/car_workvan.schem");
static CAMPER: &[u8] = include_bytes!("../../assets/structures/car_camper.schem");
static PICKUP: &[u8] = include_bytes!("../../assets/structures/car_pickup.schem");
static SUV: &[u8] = include_bytes!("../../assets/structures/car_suv.schem");
static SEDAN: &[u8] = include_bytes!("../../assets/structures/car_sedan.schem");

/// Chance (percent) that a parking space holds a car.
const OCCUPANCY_PERCENT: u64 = 50;

/// Schematic plus the quarter-turn that aligns its length with the Z axis.
fn cars() -> &'static [(StructureSchematic, u8)] {
    static CELL: OnceLock<Vec<(StructureSchematic, u8)>> = OnceLock::new();
    CELL.get_or_init(|| {
        [
            FEDEX,
            HOTROD_WHITE,
            HOTROD_BLUE,
            POLICE,
            UHAUL,
            WORKVAN,
            CAMPER,
            PICKUP,
            SUV,
            SEDAN,
        ]
        .iter()
        .filter_map(|bytes| match load_structure(bytes) {
            Ok(s) => {
                let align = if s.width > s.length { 1 } else { 0 };
                Some((s.centered(), align))
            }
            Err(e) => {
                eprintln!("car load failed: {e}");
                None
            }
        })
        .collect()
    })
}

/// Sometimes parks a random car at a space centre, aligned via `rot_base` with a random nose flip.
pub fn maybe_place_car(editor: &mut WorldEditor, cx: i32, cz: i32, rot_base: u8) {
    let base_y = editor.get_absolute_y(cx, 1, cz);
    park(
        editor,
        cx,
        cz,
        base_y,
        rot_base,
        coord_hash(cx, cz),
        i32::MAX,
    );
}

/// Like `maybe_place_car`, on a deck whose surface is `base_y`, with only cars at most
/// `max_height` blocks tall. Decks stacked over one another draw their own cars.
pub fn maybe_place_car_on_deck(
    editor: &mut WorldEditor,
    cx: i32,
    cz: i32,
    base_y: i32,
    rot_base: u8,
    max_height: i32,
) {
    let h = coord_hash(cx ^ base_y.wrapping_mul(0x2F1B), cz);
    park(editor, cx, cz, base_y, rot_base, h, max_height);
}

fn car_height(car: &StructureSchematic) -> i32 {
    car.voxels.iter().map(|v| v.1 + 1).max().unwrap_or(0)
}

fn park(
    editor: &mut WorldEditor,
    cx: i32,
    cz: i32,
    base_y: i32,
    rot_base: u8,
    h: u64,
    max_height: i32,
) {
    if !editor.place_prop(super::Prop::Car) || h % 100 >= OCCUPANCY_PERCENT {
        return;
    }
    if editor.parked_car_models() {
        park_model(editor, cx, cz, base_y, rot_base, h, max_height);
        return;
    }
    let pool: Vec<&(StructureSchematic, u8)> = cars()
        .iter()
        .filter(|(car, _)| car_height(car) <= max_height)
        .collect();
    if pool.is_empty() {
        return;
    }
    let (schem, align) = pool[((h >> 8) % pool.len() as u64) as usize];
    let flip = if (h >> 16) & 1 == 0 { 0 } else { 2 };
    let rot = (rot_base + align + flip) & 3;
    place_structure(editor, schem, cx, cz, base_y, rot, None);
}

/// Parks one of the moving cars' models: one display, at 1:1 as the spaces are, its lamps
/// dark. Not quite square in its space, as parked cars are.
fn park_model(
    editor: &mut WorldEditor,
    cx: i32,
    cz: i32,
    base_y: i32,
    rot_base: u8,
    h: u64,
    max_height: i32,
) {
    use crate::vehicle_models::{display_nbt, parked_item_model};
    use fastnbt::Value;
    let headroom = if max_height == i32::MAX {
        f64::MAX
    } else {
        f64::from(max_height) - 0.1
    };
    let Some(look) = crate::element_processing::cars::parked_look(h >> 8, headroom) else {
        return;
    };
    let unit = |shift: u32| ((h >> shift) & 0xff) as f64 / 255.0 - 0.5;
    let flip = if (h >> 16) & 1 == 0 { 0.0 } else { 180.0 };
    let yaw = f64::from(rot_base & 3) * 90.0 + flip + unit(40) * 5.0;
    let (x, z) = (
        f64::from(cx) + 0.5 + unit(48) * 0.3,
        f64::from(cz) + 0.5 + unit(56) * 0.3,
    );
    let mut nbt = display_nbt(look.model, &parked_item_model(&look), 1.0, false);
    nbt.insert(
        "Rotation".to_string(),
        Value::List(vec![Value::Float(yaw as f32), Value::Float(0.0)]),
    );
    nbt.insert(
        "Tags".to_string(),
        Value::List(vec![Value::String("arnis.parked".to_string())]),
    );
    nbt.insert("view_range".to_string(), Value::Float(PARKED_VIEW_RANGE));
    editor.add_entity_exact(
        "minecraft:item_display",
        x,
        f64::from(base_y),
        z,
        (h >> 1) as i64,
        nbt,
    );
}

/// Parked cars are drawn out to 64 blocks times this: far enough across a lot, and a big
/// lot does not draw cars nobody can make out.
const PARKED_VIEW_RANGE: f32 = 2.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_car_assets_parse() {
        assert_eq!(cars().len(), 10);
        for (car, _) in cars() {
            assert!(!car.voxels.is_empty());
            assert!(car.max_extent < 16);
        }
    }

    #[test]
    fn some_cars_fit_under_a_parking_deck() {
        let low = cars().iter().filter(|(c, _)| car_height(c) <= 3).count();
        assert!(low >= 3, "only {low} cars fit three blocks of headroom");
    }
}
