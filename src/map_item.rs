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
// Fallback when the world DataVersion cannot be read.
const DATA_VERSION: i32 = crate::world_editor::java::DATA_VERSION;

/// arnismc.com branding image, placed as a locked map at spawn.
static BRANDING_MAP_PNG: &[u8] = include_bytes!("../assets/branding/arnismc_map.png");

/// Fixed decal map ids, reserved after the preview (0), branding (1), and decal maps.
pub const BUS_STOP_MAP_ID: i32 = 2;
pub const RECYCLING_MAP_ID: i32 = 3;
pub const HYDRANT_MAP_ID: i32 = 4;
static BUS_STOP_PNG: &[u8] = include_bytes!("../assets/decorations/bus_stop.png");
static RECYCLING_PNG: &[u8] = include_bytes!("../assets/decorations/recycling.png");
static HYDRANT_PNG: &[u8] = include_bytes!("../assets/decorations/hydrant.png");

// The map must carry the same DataVersion as the world so a newer client upgrades it
// with the rest of the save rather than treating it as a stale file.
fn world_data_version(world_path: &Path) -> i32 {
    if let Ok(Value::Compound(root)) = read_gzip_nbt(&world_path.join("level.dat")) {
        if let Some(Value::Compound(data)) = root.get("Data") {
            if let Some(Value::Int(v)) = data.get("DataVersion") {
                return *v;
            }
        }
    }
    DATA_VERSION
}

/// Reads the world spawn XZ from level.dat so callers can align features with it.
pub fn read_spawn_xz(world_path: &Path) -> Option<(i32, i32)> {
    if let Ok(Value::Compound(root)) = read_gzip_nbt(&world_path.join("level.dat")) {
        if let Some(Value::Compound(data)) = root.get("Data") {
            if let (Some(Value::Int(x)), Some(Value::Int(z))) =
                (data.get("SpawnX"), data.get("SpawnZ"))
            {
                return Some((*x, *z));
            }
        }
    }
    None
}

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

// Next free map id; respects existing counter files so user maps are never clobbered.
// 26.1 renamed idcounts.dat to last_id.dat, so check both.
fn next_map_id(data_dir: &Path) -> i32 {
    let mut highest: Option<i32> = None;
    for name in ["idcounts.dat", "last_id.dat"] {
        if let Ok(Value::Compound(root)) = read_gzip_nbt(&data_dir.join(name)) {
            if let Some(Value::Compound(data)) = root.get("data") {
                if let Some(Value::Int(n)) = data.get("map") {
                    highest = Some(highest.map_or(*n, |h| h.max(*n)));
                }
            }
        }
    }
    highest.map_or(0, |h| h + 1)
}

fn build_map_dat(
    colors: Vec<i8>,
    scale: i8,
    tracking: bool,
    x_center: i32,
    z_center: i32,
    data_version: i32,
) -> Value {
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
    root.insert("DataVersion".to_string(), Value::Int(data_version));
    root.insert("data".to_string(), Value::Compound(data));
    Value::Compound(root)
}

fn build_idcounts(map_id: i32, data_version: i32) -> Value {
    let mut data = HashMap::new();
    data.insert("map".to_string(), Value::Int(map_id));
    let mut root = HashMap::new();
    root.insert("DataVersion".to_string(), Value::Int(data_version));
    root.insert("data".to_string(), Value::Compound(data));
    Value::Compound(root)
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

fn item_slot(entry: &Value) -> Option<i8> {
    match entry {
        Value::Compound(m) => match m.get("Slot") {
            Some(Value::Byte(s)) => Some(*s),
            _ => None,
        },
        _ => None,
    }
}

// Puts the map into slot 0, only ever replacing a filled map there; other items
// (including the player's own maps in other slots) are left untouched. If slot 0
// holds something else, the map goes into the first free slot instead.
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
        items.retain(|e| !(is_filled_map(e) && item_slot(e) == Some(0)));
        let slot = if items.iter().any(|e| item_slot(e) == Some(0)) {
            (0..36i8)
                .find(|s| !items.iter().any(|e| item_slot(e) == Some(*s)))
                .ok_or("player inventory is full")?
        } else {
            0
        };
        items.push(map_item_entry(map_id, slot));
    }
    write_gzip_nbt(&level_path, &root)
}

// Write the map .dat under both the pre- and post-26.1 filenames.
fn write_map_dat_files(data_dir: &Path, map_id: i32, map_dat: &Value) -> Result<(), String> {
    write_gzip_nbt(&data_dir.join(format!("map_{map_id}.dat")), map_dat)?;
    write_gzip_nbt(&data_dir.join(format!("{map_id}.dat")), map_dat)
}

// Quantize a bundled PNG to a locked 128x128 map.
fn image_map_dat(png: &[u8], data_version: i32) -> Result<Value, String> {
    let img = image::load_from_memory(png)
        .map_err(|e| format!("decode image: {e}"))?
        .to_rgb8();
    let img = if img.width() == MAP_SIZE as u32 && img.height() == MAP_SIZE as u32 {
        img
    } else {
        image::imageops::resize(
            &img,
            MAP_SIZE as u32,
            MAP_SIZE as u32,
            image::imageops::FilterType::Triangle,
        )
    };

    let mut colors = vec![TRANSPARENT as i8; (MAP_SIZE * MAP_SIZE) as usize];
    for j in 0..MAP_SIZE {
        for i in 0..MAP_SIZE {
            let p = img.get_pixel(i as u32, j as u32);
            colors[(j * MAP_SIZE + i) as usize] = nearest_map_color(p.0[0], p.0[1], p.0[2]) as i8;
        }
    }
    Ok(build_map_dat(colors, 0, false, 0, 0, data_version))
}

// Like image_map_dat but infallible: a decode error yields a blank map so a reserved id
// always has a file and its item frame can't point at a missing map.
fn image_map_dat_or_blank(png: &[u8], data_version: i32) -> Value {
    image_map_dat(png, data_version).unwrap_or_else(|e| {
        eprintln!("Warning: map image decode failed ({e}); using a blank map");
        build_map_dat(
            vec![TRANSPARENT as i8; (MAP_SIZE * MAP_SIZE) as usize],
            0,
            false,
            0,
            0,
            data_version,
        )
    })
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

    let data_version = world_data_version(world_path);
    let data_dir = world_path.join("data");
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create data dir: {e}"))?;
    let map_id = next_map_id(&data_dir);

    let map_dat = build_map_dat(colors, scale, tracking, x_center, z_center, data_version);
    write_map_dat_files(&data_dir, map_id, &map_dat)?;

    // Branding map is id+1; a decode failure yields a blank map so the frame never breaks.
    let branding_id = map_id + 1;
    let branding_dat = image_map_dat_or_blank(BRANDING_MAP_PNG, data_version);
    write_map_dat_files(&data_dir, branding_id, &branding_dat)?;

    let idcounts = build_idcounts(branding_id, data_version);
    write_gzip_nbt(&data_dir.join("idcounts.dat"), &idcounts)?;
    write_gzip_nbt(&data_dir.join("last_id.dat"), &idcounts)?;

    // Only the preview goes in the hotbar; branding is world-only.
    insert_into_inventory(world_path, map_id)
}

/// Writes only the arnismc.com branding map (the world's first map) when the preview map is off.
pub fn write_branding_map_only(world_path: &Path) -> Result<(), String> {
    let data_version = world_data_version(world_path);
    let data_dir = world_path.join("data");
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create data dir: {e}"))?;
    let map_id = next_map_id(&data_dir);

    let branding_dat = image_map_dat_or_blank(BRANDING_MAP_PNG, data_version);
    write_map_dat_files(&data_dir, map_id, &branding_dat)?;

    let idcounts = build_idcounts(map_id, data_version);
    write_gzip_nbt(&data_dir.join("idcounts.dat"), &idcounts)?;
    write_gzip_nbt(&data_dir.join("last_id.dat"), &idcounts)?;
    Ok(())
}

/// Writes the fixed decal maps so their in-world frames resolve.
pub fn write_decoration_maps(world_path: &Path) -> Result<(), String> {
    let data_version = world_data_version(world_path);
    let data_dir = world_path.join("data");
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("create data dir: {e}"))?;

    let mut highest = next_map_id(&data_dir) - 1;
    for (id, png) in [
        (BUS_STOP_MAP_ID, BUS_STOP_PNG),
        (RECYCLING_MAP_ID, RECYCLING_PNG),
        (HYDRANT_MAP_ID, HYDRANT_PNG),
    ] {
        write_map_dat_files(&data_dir, id, &image_map_dat_or_blank(png, data_version))?;
        highest = highest.max(id);
    }

    let idcounts = build_idcounts(highest, data_version);
    write_gzip_nbt(&data_dir.join("idcounts.dat"), &idcounts)?;
    write_gzip_nbt(&data_dir.join("last_id.dat"), &idcounts)?;
    Ok(())
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
        let world = std::path::PathBuf::from(
            crate::world_utils::create_new_world(tmp.path(), None).unwrap(),
        );
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
        // Preview is map 0; the branding map reserves id 1, so the counter ends at 1.
        assert_eq!(iddata.get("map"), Some(&Value::Int(1)));

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
    fn writes_branding_map_beside_preview_but_world_only() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let preview = PreviewAccumulator::new(&xzbbox);
        write_map_item(&world, &preview, &xzbbox).unwrap();

        // Preview is map 0; the arnismc.com branding map is the next id, 1.
        let Value::Compound(root) = read_gzip_nbt(&world.join("data/map_1.dat")).unwrap() else {
            panic!("branding map root");
        };
        let Some(Value::Compound(data)) = root.get("data") else {
            panic!("branding data");
        };
        assert_eq!(data.get("locked"), Some(&Value::Byte(1)));
        // Fixed art, not terrain: the player marker is disabled.
        assert_eq!(data.get("trackingPosition"), Some(&Value::Byte(0)));
        let Some(Value::ByteArray(colors)) = data.get("colors") else {
            panic!("branding colors");
        };
        assert_eq!(colors.len(), 16384);
        // The bundled art is not blank, so at least some pixels are opaque.
        assert!(colors.iter().any(|&c| c != TRANSPARENT as i8));

        // Branding stays out of the hotbar: only the preview map (id 0) is held.
        let items = inventory_items(&world);
        assert_eq!(items.iter().filter(|e| is_filled_map(e)).count(), 1);
    }

    #[test]
    fn branding_only_writes_map_zero_and_no_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        write_branding_map_only(&world).unwrap();

        // With the preview off, the branding map is the world's first map (id 0).
        let Value::Compound(root) = read_gzip_nbt(&world.join("data/map_0.dat")).unwrap() else {
            panic!("branding map root");
        };
        let Some(Value::Compound(data)) = root.get("data") else {
            panic!("branding data");
        };
        assert_eq!(data.get("locked"), Some(&Value::Byte(1)));
        let Some(Value::ByteArray(colors)) = data.get("colors") else {
            panic!("branding colors");
        };
        assert_eq!(colors.len(), 16384);

        let Value::Compound(idroot) = read_gzip_nbt(&world.join("data/idcounts.dat")).unwrap()
        else {
            panic!("idcounts root");
        };
        let Some(Value::Compound(iddata)) = idroot.get("data") else {
            panic!("idcounts data");
        };
        assert_eq!(iddata.get("map"), Some(&Value::Int(0)));

        // Preview-off path never touches the hotbar.
        assert_eq!(
            inventory_items(&world)
                .iter()
                .filter(|e| is_filled_map(e))
                .count(),
            0
        );
    }

    #[test]
    fn writes_decoration_maps_with_reserved_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            std::path::PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());
        write_decoration_maps(&world).unwrap();

        for id in [BUS_STOP_MAP_ID, RECYCLING_MAP_ID, HYDRANT_MAP_ID] {
            let Value::Compound(root) =
                read_gzip_nbt(&world.join(format!("data/map_{id}.dat"))).unwrap()
            else {
                panic!("decal map {id} root");
            };
            let Some(Value::Compound(data)) = root.get("data") else {
                panic!("decal map {id} data");
            };
            assert_eq!(data.get("locked"), Some(&Value::Byte(1)));
            let Some(Value::ByteArray(colors)) = data.get("colors") else {
                panic!("decal map {id} colors");
            };
            assert_eq!(colors.len(), 16384);
        }

        // idcounts must reach the highest reserved id so user maps never overwrite the decals.
        let Value::Compound(idroot) = read_gzip_nbt(&world.join("data/idcounts.dat")).unwrap()
        else {
            panic!("idcounts root");
        };
        let Some(Value::Compound(iddata)) = idroot.get("data") else {
            panic!("idcounts data");
        };
        assert_eq!(iddata.get("map"), Some(&Value::Int(HYDRANT_MAP_ID)));
    }

    fn inventory_items(world: &std::path::Path) -> Vec<Value> {
        let Value::Compound(level) = read_gzip_nbt(&world.join("level.dat")).unwrap() else {
            panic!("level root");
        };
        let Some(Value::Compound(ldata)) = level.get("Data") else {
            panic!("level data");
        };
        let Some(Value::Compound(player)) = ldata.get("Player") else {
            panic!("player");
        };
        match player.get("Inventory") {
            Some(Value::List(items)) => items.clone(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn oversized_world_disables_the_player_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let world = std::path::PathBuf::from(
            crate::world_utils::create_new_world(tmp.path(), None).unwrap(),
        );
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
    fn preserves_user_items_and_dodges_occupied_slot_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let world = std::path::PathBuf::from(
            crate::world_utils::create_new_world(tmp.path(), None).unwrap(),
        );

        // Seed: a sword in slot 0 and the user's own map in slot 5.
        let mut root = read_gzip_nbt(&world.join("level.dat")).unwrap();
        if let Value::Compound(ref mut r) = root {
            if let Some(Value::Compound(ref mut data)) = r.get_mut("Data") {
                if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                    let mut sword = HashMap::new();
                    sword.insert("Slot".to_string(), Value::Byte(0));
                    sword.insert(
                        "id".to_string(),
                        Value::String("minecraft:iron_sword".to_string()),
                    );
                    player.insert(
                        "Inventory".to_string(),
                        Value::List(vec![Value::Compound(sword), map_item_entry(99, 5)]),
                    );
                }
            }
        }
        write_gzip_nbt(&world.join("level.dat"), &root).unwrap();

        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let preview = PreviewAccumulator::new(&xzbbox);
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
        // Sword untouched, user map untouched, our map in the first free slot.
        assert!(items
            .iter()
            .any(|e| item_slot(e) == Some(0) && !is_filled_map(e)));
        assert!(items
            .iter()
            .any(|e| item_slot(e) == Some(5) && is_filled_map(e)));
        assert!(items
            .iter()
            .any(|e| item_slot(e) == Some(1) && is_filled_map(e)));
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn respects_existing_idcounts_and_replaces_old_item() {
        let tmp = tempfile::tempdir().unwrap();
        let world = std::path::PathBuf::from(
            crate::world_utils::create_new_world(tmp.path(), None).unwrap(),
        );
        let data_dir = world.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        write_gzip_nbt(
            &data_dir.join("idcounts.dat"),
            &build_idcounts(5, DATA_VERSION),
        )
        .unwrap();

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
