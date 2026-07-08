//! Writes a locked filled-map item showing the whole generated world (Java only).

use crate::coordinate_system::cartesian::XZBBox;
use crate::map_item_palette::{nearest_map_color, TRANSPARENT};
use crate::map_renderer::PreviewAccumulator;
use fastnbt::{ByteArray, Value};
use flate2::read::GzDecoder;
use image::RgbImage;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

const MAP_SIZE: i32 = 128;
const DATA_VERSION: i32 = 4189;

fn read_gzip_nbt(path: &Path) -> Result<Value, String> {
    let raw = std::fs::read(path).map_err(|e| format!("read {path:?}: {e}"))?;
    let mut decompressed = Vec::new();
    GzDecoder::new(raw.as_slice())
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("decompress {path:?}: {e}"))?;
    fastnbt::from_bytes(&decompressed).map_err(|e| format!("parse {path:?}: {e}"))
}

fn write_gzip_nbt(path: &Path, value: &Value) -> Result<(), String> {
    let serialized = fastnbt::to_bytes(value).map_err(|e| format!("serialize {path:?}: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("compress {path:?}: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("finish {path:?}: {e}"))?;
    std::fs::write(path, compressed).map_err(|e| format!("write {path:?}: {e}"))
}

// Map geometry: blocks per map pixel, scale byte, and whether the player marker
// stays accurate. Oversized worlds exceed vanilla's 16-blocks/px dot mapping, so
// the marker is disabled there rather than shown misaligned.
fn map_geometry(max_dim: i32) -> (i32, i8, bool) {
    for s in 0..=4i32 {
        if MAP_SIZE << s >= max_dim {
            return (1 << s, s as i8, true);
        }
    }
    ((max_dim + MAP_SIZE - 1) / MAP_SIZE, 4, false)
}

// Quantized 128x128 colors: average the preview pixels under each map pixel's footprint.
#[allow(clippy::too_many_arguments)]
fn build_colors(
    img: &RgbImage,
    img_min_x: i32,
    img_min_z: i32,
    step: u32,
    xzbbox: &XZBBox,
    bpp: i32,
    x_center: i32,
    z_center: i32,
) -> Vec<i8> {
    let step = step.max(1) as i32;
    let mut colors = vec![TRANSPARENT as i8; (MAP_SIZE * MAP_SIZE) as usize];
    for j in 0..MAP_SIZE {
        for i in 0..MAP_SIZE {
            let wx0 = x_center + (i - 64) * bpp;
            let wz0 = z_center + (j - 64) * bpp;
            let wx1 = wx0 + bpp - 1;
            let wz1 = wz0 + bpp - 1;
            if wx1 < xzbbox.min_x()
                || wx0 > xzbbox.max_x()
                || wz1 < xzbbox.min_z()
                || wz0 > xzbbox.max_z()
            {
                continue;
            }
            let px0 =
                ((wx0.max(xzbbox.min_x()) - img_min_x) / step).clamp(0, img.width() as i32 - 1);
            let px1 =
                ((wx1.min(xzbbox.max_x()) - img_min_x) / step).clamp(0, img.width() as i32 - 1);
            let pz0 =
                ((wz0.max(xzbbox.min_z()) - img_min_z) / step).clamp(0, img.height() as i32 - 1);
            let pz1 =
                ((wz1.min(xzbbox.max_z()) - img_min_z) / step).clamp(0, img.height() as i32 - 1);
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
            for pz in pz0..=pz1 {
                for px in px0..=px1 {
                    let p = img.get_pixel(px as u32, pz as u32);
                    r += p.0[0] as u64;
                    g += p.0[1] as u64;
                    b += p.0[2] as u64;
                    n += 1;
                }
            }
            if let (Some(ar), Some(ag), Some(ab)) =
                (r.checked_div(n), g.checked_div(n), b.checked_div(n))
            {
                let id = nearest_map_color(ar as u8, ag as u8, ab as u8);
                colors[(j * MAP_SIZE + i) as usize] = id as i8;
            }
        }
    }
    colors
}

// Next free map id; respects an existing idcounts.dat so user maps are never clobbered.
fn next_map_id(data_dir: &Path) -> i32 {
    let path = data_dir.join("idcounts.dat");
    if let Ok(Value::Compound(root)) = read_gzip_nbt(&path) {
        if let Some(Value::Compound(data)) = root.get("data") {
            if let Some(Value::Int(n)) = data.get("map") {
                return n + 1;
            }
        }
    }
    0
}

fn write_map_dat(
    path: &Path,
    colors: Vec<i8>,
    scale: i8,
    tracking: bool,
    x_center: i32,
    z_center: i32,
) -> Result<(), String> {
    let mut data = HashMap::new();
    data.insert("scale".to_string(), Value::Byte(scale));
    data.insert(
        "dimension".to_string(),
        Value::String("minecraft:overworld".to_string()),
    );
    data.insert("trackingPosition".to_string(), Value::Byte(tracking as i8));
    data.insert("unlimitedTracking".to_string(), Value::Byte(0));
    data.insert("locked".to_string(), Value::Byte(1));
    data.insert("xCenter".to_string(), Value::Int(x_center));
    data.insert("zCenter".to_string(), Value::Int(z_center));
    data.insert(
        "colors".to_string(),
        Value::ByteArray(ByteArray::new(colors)),
    );
    let mut root = HashMap::new();
    root.insert("DataVersion".to_string(), Value::Int(DATA_VERSION));
    root.insert("data".to_string(), Value::Compound(data));
    write_gzip_nbt(path, &Value::Compound(root))
}

fn write_idcounts(path: &Path, map_id: i32) -> Result<(), String> {
    let mut data = HashMap::new();
    data.insert("map".to_string(), Value::Int(map_id));
    let mut root = HashMap::new();
    root.insert("DataVersion".to_string(), Value::Int(DATA_VERSION));
    root.insert("data".to_string(), Value::Compound(data));
    write_gzip_nbt(path, &Value::Compound(root))
}

fn map_item_entry(map_id: i32, slot: i8) -> Value {
    let mut components = HashMap::new();
    components.insert("minecraft:map_id".to_string(), Value::Int(map_id));
    let mut item = HashMap::new();
    item.insert("Slot".to_string(), Value::Byte(slot));
    item.insert(
        "id".to_string(),
        Value::String("minecraft:filled_map".to_string()),
    );
    // 1.20.5+ item format: lowercase count (Int) with components, not Count (Byte) + tag.
    item.insert("count".to_string(), Value::Int(1));
    item.insert("components".to_string(), Value::Compound(components));
    Value::Compound(item)
}

fn is_filled_map(entry: &Value) -> bool {
    matches!(entry, Value::Compound(m)
        if matches!(m.get("id"), Some(Value::String(s)) if s == "minecraft:filled_map"))
}

// Puts the map into the player's inventory, replacing any filled map from a previous run.
fn insert_into_inventory(world_path: &Path, map_id: i32) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    let mut root = read_gzip_nbt(&level_path)?;
    {
        let Value::Compound(ref mut r) = root else {
            return Err("level.dat root is not a compound".to_string());
        };
        let Some(Value::Compound(ref mut data)) = r.get_mut("Data") else {
            return Err("level.dat missing Data compound".to_string());
        };
        let Some(Value::Compound(ref mut player)) = data.get_mut("Player") else {
            return Err("level.dat missing Player compound".to_string());
        };
        let inventory = player
            .entry("Inventory".to_string())
            .or_insert_with(|| Value::List(Vec::new()));
        let Value::List(ref mut items) = inventory else {
            return Err("Player.Inventory is not a list".to_string());
        };
        items.retain(|e| !is_filled_map(e));
        // Slot 0: first hotbar slot.
        items.push(map_item_entry(map_id, 0));
    }
    write_gzip_nbt(&level_path, &root)
}

// Renders the preview into a locked map covering the entire world and hands it to the player.
pub fn write_map_item(
    world_path: &Path,
    preview: &PreviewAccumulator,
    xzbbox: &XZBBox,
) -> Result<(), String> {
    let w = xzbbox.max_x() - xzbbox.min_x() + 1;
    let h = xzbbox.max_z() - xzbbox.min_z() + 1;
    let (bpp, scale, tracking) = map_geometry(w.max(h));
    let x_center = xzbbox.min_x() + w / 2;
    let z_center = xzbbox.min_z() + h / 2;

    let img = preview.render_image();
    if img.width() == 0 || img.height() == 0 {
        return Err("empty preview image".to_string());
    }
    let colors = build_colors(
        &img,
        preview.min_x(),
        preview.min_z(),
        preview.step(),
        xzbbox,
        bpp,
        x_center,
        z_center,
    );

    let data_dir = world_path.join("data");
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create data dir: {e}"))?;
    let map_id = next_map_id(&data_dir);
    write_map_dat(
        &data_dir.join(format!("map_{map_id}.dat")),
        colors,
        scale,
        tracking,
        x_center,
        z_center,
    )?;
    write_idcounts(&data_dir.join("idcounts.dat"), map_id)?;
    insert_into_inventory(world_path, map_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_scales_with_world_size() {
        assert_eq!(map_geometry(100), (1, 0, true));
        assert_eq!(map_geometry(200), (2, 1, true));
        assert_eq!(map_geometry(2048), (16, 4, true));
        // Oversized worlds get a custom fit; the marker would misalign, so it's off.
        assert_eq!(map_geometry(3000), (24, 4, false));
    }

    #[test]
    fn writes_map_files_and_inventory_item() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        let xzbbox = XZBBox::rect_from_xz_lengths(300.0, 100.0).unwrap();
        let preview = PreviewAccumulator::new(&xzbbox);
        write_map_item(&world, &preview, &xzbbox).unwrap();

        let Value::Compound(root) = read_gzip_nbt(&world.join("data/map_0.dat")).unwrap() else {
            panic!("map root");
        };
        let Some(Value::Compound(data)) = root.get("data") else {
            panic!("map data");
        };
        assert_eq!(data.get("locked"), Some(&Value::Byte(1)));
        assert_eq!(data.get("scale"), Some(&Value::Byte(2)));
        assert_eq!(data.get("trackingPosition"), Some(&Value::Byte(1)));
        let Some(Value::ByteArray(colors)) = data.get("colors") else {
            panic!("colors");
        };
        assert_eq!(colors.len(), 16384);
        // Non-square world: center sampled, area past the short axis stays transparent.
        assert_ne!(colors[64 * 128 + 64], TRANSPARENT as i8);
        assert_eq!(colors[(64 + 40) * 128 + 64], TRANSPARENT as i8);

        let Value::Compound(idroot) = read_gzip_nbt(&world.join("data/idcounts.dat")).unwrap()
        else {
            panic!("idcounts root");
        };
        let Some(Value::Compound(iddata)) = idroot.get("data") else {
            panic!("idcounts data");
        };
        assert_eq!(iddata.get("map"), Some(&Value::Int(0)));

        let Value::Compound(level) = read_gzip_nbt(&world.join("level.dat")).unwrap() else {
            panic!("level root");
        };
        let Some(Value::Compound(ldata)) = level.get("Data") else {
            panic!("level data");
        };
        let Some(Value::Compound(player)) = ldata.get("Player") else {
            panic!("player");
        };
        let Some(Value::List(items)) = player.get("Inventory") else {
            panic!("inventory");
        };
        assert!(items.iter().any(is_filled_map));
    }

    #[test]
    fn oversized_world_disables_the_player_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        let xzbbox = XZBBox::rect_from_xz_lengths(3000.0, 3000.0).unwrap();
        let preview = PreviewAccumulator::new(&xzbbox);
        write_map_item(&world, &preview, &xzbbox).unwrap();

        let Value::Compound(root) = read_gzip_nbt(&world.join("data/map_0.dat")).unwrap() else {
            panic!("map root");
        };
        let Some(Value::Compound(data)) = root.get("data") else {
            panic!("map data");
        };
        assert_eq!(data.get("trackingPosition"), Some(&Value::Byte(0)));
        assert_eq!(data.get("scale"), Some(&Value::Byte(4)));
    }

    #[test]
    fn respects_existing_idcounts_and_replaces_old_item() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        let data_dir = world.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        write_idcounts(&data_dir.join("idcounts.dat"), 5).unwrap();

        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let preview = PreviewAccumulator::new(&xzbbox);
        write_map_item(&world, &preview, &xzbbox).unwrap();
        assert!(data_dir.join("map_6.dat").exists());

        // A second run must not stack a second map item.
        write_map_item(&world, &preview, &xzbbox).unwrap();
        let Value::Compound(level) = read_gzip_nbt(&world.join("level.dat")).unwrap() else {
            panic!("level root");
        };
        let Some(Value::Compound(ldata)) = level.get("Data") else {
            panic!("level data");
        };
        let Some(Value::Compound(player)) = ldata.get("Player") else {
            panic!("player");
        };
        let Some(Value::List(items)) = player.get("Inventory") else {
            panic!("inventory");
        };
        assert_eq!(items.iter().filter(|e| is_filled_map(e)).count(), 1);
    }
}
