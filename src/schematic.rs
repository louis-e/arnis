//! Sponge `.schem` loader. Parses a gzipped WorldEdit/Sponge schematic (v2 or v3)
//! into a list of `(dx, dy, dz, Block)` voxels for stamping trees. Only logs and
//! leaves are kept; ground cover, vines, cocoa, pale-garden blocks, air, and unknown
//! blocks are dropped.

use std::collections::HashMap;
use std::io::Read;

use fastnbt::Value;

use crate::block_definitions::{
    Block, ACACIA_LEAVES, ACACIA_LOG, AZALEA_LEAVES, BIRCH_LEAVES, BIRCH_LOG, CHERRY_LEAVES,
    CHERRY_LOG, DARK_OAK_LEAVES, DARK_OAK_LOG, JUNGLE_LEAVES, JUNGLE_LOG, MANGROVE_LEAVES,
    MANGROVE_LOG, OAK_LEAVES, OAK_LOG, SPRUCE_LEAVES, SPRUCE_LOG, WATER,
};
use crate::world_editor::WorldEditor;

/// A parsed schematic: dimensions plus the non-air log/leaf voxels. The origin is the
/// schematic's min corner (`x` in `0..width`, `y` in `0..height`, `z` in `0..length`).
pub struct Schematic {
    pub width: i32,
    pub height: i32,
    pub length: i32,
    pub voxels: Vec<(i32, i32, i32, Block)>,
    /// Lowest log voxel y (the trunk floor); precomputed for the root pass.
    pub min_log_vy: i32,
}

/// Map a Minecraft block-state string to one of our blocks, or `None` for anything we
/// intentionally drop (air, ground cover, vines, cocoa, pale-garden markers) or do not know.
pub fn map_block(name: &str) -> Option<Block> {
    let base = name
        .split('[')
        .next()
        .unwrap_or(name)
        .trim_start_matches("minecraft:");
    let block = match base {
        "oak_log" | "oak_wood" => OAK_LOG,
        "birch_log" | "birch_wood" => BIRCH_LOG,
        "spruce_log" | "spruce_wood" => SPRUCE_LOG,
        "dark_oak_log" | "dark_oak_wood" => DARK_OAK_LOG,
        "jungle_log" | "jungle_wood" => JUNGLE_LOG,
        "acacia_log" | "acacia_wood" => ACACIA_LOG,
        "cherry_log" | "cherry_wood" => CHERRY_LOG,
        "mangrove_log" | "mangrove_wood" => MANGROVE_LOG,
        "oak_leaves" => OAK_LEAVES,
        "birch_leaves" => BIRCH_LEAVES,
        "spruce_leaves" => SPRUCE_LEAVES,
        "dark_oak_leaves" => DARK_OAK_LEAVES,
        "jungle_leaves" => JUNGLE_LEAVES,
        "acacia_leaves" => ACACIA_LEAVES,
        "cherry_leaves" => CHERRY_LEAVES,
        "mangrove_leaves" => MANGROVE_LEAVES,
        "azalea_leaves" | "flowering_azalea_leaves" => AZALEA_LEAVES,
        // Foliage some models use as canopy (e.g. red alder builds its crown from vine);
        // rendered as oak-green leaves so the canopy isn't dropped.
        "vine" | "moss_block" | "moss_carpet" => OAK_LEAVES,
        // Stripped logs: no stripped block ids, so use the matching base log.
        "stripped_oak_log" | "stripped_oak_wood" => OAK_LOG,
        "stripped_birch_log" | "stripped_birch_wood" => BIRCH_LOG,
        "stripped_spruce_log" | "stripped_spruce_wood" => SPRUCE_LOG,
        "stripped_dark_oak_log" | "stripped_dark_oak_wood" => DARK_OAK_LOG,
        "stripped_jungle_log" | "stripped_jungle_wood" => JUNGLE_LOG,
        "stripped_acacia_log" | "stripped_acacia_wood" => ACACIA_LOG,
        "stripped_cherry_log" | "stripped_cherry_wood" => CHERRY_LOG,
        "stripped_mangrove_log" | "stripped_mangrove_wood" => MANGROVE_LOG,
        // Pale-oak wood is used as pale trunk/foliage in real trees, not the pale garden.
        "pale_oak_log" | "pale_oak_wood" | "stripped_pale_oak_log" | "stripped_pale_oak_wood" => {
            BIRCH_LOG
        }
        "pale_oak_leaves" => BIRCH_LEAVES,
        "mangrove_roots" | "muddy_mangrove_roots" => MANGROVE_LOG,
        "mangrove_propagule" => MANGROVE_LEAVES,
        "bamboo_block" | "stripped_bamboo_block" => JUNGLE_LOG,
        "warped_stem" | "stripped_warped_stem" | "warped_hyphae" | "stripped_warped_hyphae" => {
            SPRUCE_LOG
        }
        _ => return None,
    };
    Some(block)
}

/// Decode a Sponge `BlockData` byte stream: LEB128 varint palette indices.
fn decode_varints(data: &[u8]) -> Vec<i32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let mut val: i32 = 0;
        let mut shift = 0u32;
        loop {
            let byte = data[i];
            i += 1;
            // Guard the shift so an over-long/corrupt varint can't panic (shift >= 32);
            // we keep consuming continuation bytes to stay aligned, just stop accumulating.
            if shift < 32 {
                val |= i32::from(byte & 0x7F) << shift;
            }
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if i >= data.len() {
                break;
            }
        }
        out.push(val);
    }
    out
}

fn as_compound(v: &Value) -> Option<&HashMap<String, Value>> {
    match v {
        Value::Compound(c) => Some(c),
        _ => None,
    }
}

fn short_field(c: &HashMap<String, Value>, k: &str) -> Result<i32, String> {
    match c.get(k) {
        Some(Value::Short(s)) => Ok(i32::from(*s)),
        Some(Value::Int(i)) => Ok(*i),
        _ => Err(format!("schem: missing short field {k}")),
    }
}

/// Load a gzipped Sponge `.schem` (format v2 or v3) and return its log/leaf voxels.
pub fn load_schem(gz_bytes: &[u8]) -> Result<Schematic, String> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz_bytes)
        .read_to_end(&mut raw)
        .map_err(|e| format!("schem: gunzip failed: {e}"))?;
    let root: Value = fastnbt::from_bytes(&raw).map_err(|e| format!("schem: nbt parse: {e}"))?;
    let root_c = as_compound(&root).ok_or("schem: root not a compound")?;

    // Sponge v3 nests everything under "Schematic"; v2 keeps it at the root.
    let scm = root_c
        .get("Schematic")
        .and_then(as_compound)
        .unwrap_or(root_c);

    let width = short_field(scm, "Width")?;
    let height = short_field(scm, "Height")?;
    let length = short_field(scm, "Length")?;
    if width <= 0 || height <= 0 || length <= 0 {
        return Err("schem: non-positive dimensions".into());
    }

    // v3: Blocks { Palette, Data }; v2: root-level Palette + BlockData.
    let (palette_v, data_v) = match scm.get("Blocks").and_then(as_compound) {
        Some(blocks) => (blocks.get("Palette"), blocks.get("Data")),
        None => (scm.get("Palette"), scm.get("BlockData")),
    };
    let palette = palette_v
        .and_then(as_compound)
        .ok_or("schem: missing Palette")?;

    // Palette maps block-state string -> index; invert to index -> our Block (drops skipped).
    let mut idx_to_block: HashMap<i32, Block> = HashMap::new();
    for (name, v) in palette {
        if let Value::Int(i) = v {
            if let Some(block) = map_block(name) {
                idx_to_block.insert(*i, block);
            }
        }
    }

    let data_bytes: Vec<u8> = match data_v {
        Some(Value::ByteArray(b)) => b.iter().map(|&x| x as u8).collect(),
        _ => return Err("schem: missing BlockData".into()),
    };
    let indices = decode_varints(&data_bytes);

    let wl = width * length;
    let mut voxels = Vec::new();
    for (i, &idx) in indices.iter().enumerate() {
        let i = i as i32;
        if let Some(block) = idx_to_block.get(&idx).cloned() {
            let x = i % width;
            let z = (i / width) % length;
            let y = i / wl;
            voxels.push((x, y, z, block));
        }
    }

    // Normalise so the lowest log/leaf sits at y=0 (schems often pad air below the trunk;
    // without this the tree floats above the ground).
    if let Some(min_y) = voxels.iter().map(|v| v.1).min() {
        if min_y != 0 {
            for v in &mut voxels {
                v.1 -= min_y;
            }
        }
    }
    voxels.shrink_to_fit();

    let min_log_vy = voxels
        .iter()
        .filter(|&&(_, _, _, b)| is_log(b))
        .map(|&(_, vy, _, _)| vy)
        .min()
        .unwrap_or(0);

    Ok(Schematic {
        width,
        height,
        length,
        voxels,
        min_log_vy,
    })
}

/// Rotate a schematic cell offset `(x, z)` by `k` quarter-turns clockwise (k in 0..=3).
pub fn rotate_xz(x: i32, z: i32, w: i32, l: i32, k: u8) -> (i32, i32) {
    match k & 3 {
        0 => (x, z),
        1 => (l - 1 - z, x),
        2 => (w - 1 - x, l - 1 - z),
        _ => (z, w - 1 - x),
    }
}

/// Trunk slot with an explicit spacing `s` (blocks). Pure function of (x,z) so it is seam-safe.
pub fn trunk_slot_s(x: i32, z: i32, s: i32) -> (i32, i32) {
    let s = s.max(1);
    let cx = x.div_euclid(s);
    let cz = z.div_euclid(s);
    let h =
        crate::land_cover::coord_hash(cx.wrapping_mul(0x1f1f) + 17, cz.wrapping_mul(0x2b2b) + 91);
    let jx = (h & 1) as i32;
    let jz = ((h >> 1) & 1) as i32;
    (cx * s + jx, cz * s + jz)
}

/// One of the eight trunk log types `map_block` can emit.
fn is_log(b: Block) -> bool {
    matches!(
        b,
        OAK_LOG
            | BIRCH_LOG
            | SPRUCE_LOG
            | DARK_OAK_LOG
            | JUNGLE_LOG
            | ACACIA_LOG
            | CHERRY_LOG
            | MANGROVE_LOG
    )
}

/// A log column counts as a ground-rooted trunk base only if its lowest log is within this
/// many blocks of the schem floor; higher columns are branch tips and are never rooted.
const ROOT_BASE_VY: i32 = 2;
/// Safety bound on how far a real trunk root may reach down (only bites tile-seam deltas).
const ROOT_MAX: i32 = 64;

/// Stamp a schematic into the world with its footprint centred on `(anchor_x, anchor_z)`,
/// the base row (`y = 0`) at `base_y`, rotated by `rot` quarter-turns. Never overwrites the
/// blacklist (buildings, water). Pure function of its inputs, so it is seam-safe.
#[allow(clippy::too_many_arguments)]
pub fn place_schematic_tree(
    editor: &mut WorldEditor,
    schem: &Schematic,
    anchor_x: i32,
    anchor_z: i32,
    base_y: i32,
    rot: u8,
    blacklist: &[Block],
    footprints: Option<&crate::floodfill_cache::BuildingFootprintBitmap>,
    y_offset: i32,
) {
    let (fw, fl) = if rot & 1 == 0 {
        (schem.width, schem.length)
    } else {
        (schem.length, schem.width)
    };
    let cx = (fw - 1) / 2;
    let cz = (fl - 1) / 2;
    let min_log_vy = schem.min_log_vy;
    let mut trunk_bottom: HashMap<(i32, i32), (i32, Block)> =
        HashMap::with_capacity((schem.width * schem.length).max(0) as usize);
    for &(vx, vy, vz, block) in &schem.voxels {
        let (rx, rz) = rotate_xz(vx, vz, schem.width, schem.length, rot);
        let wx = anchor_x + rx - cx;
        let wz = anchor_z + rz - cz;
        if footprints.is_some_and(|f| f.contains(wx, wz)) {
            continue;
        }
        // Logs over placed water are always skipped; for predicted ESA water only a low
        // root-level log is skipped (high trunk/canopy logs crossing a water cell are kept so
        // bank/mangrove trunks keep no holes). Leaves always overhang.
        if is_log(block) {
            let root_level = vy <= min_log_vy + ROOT_BASE_VY;
            let over_water = editor.check_for_block(wx, 0, wz, Some(&[WATER]))
                || (root_level && editor.is_lc_water(wx, wz));
            if over_water {
                continue;
            }
        }
        editor.set_block_absolute(block, wx, base_y + vy, wz, None, Some(blacklist));
        if is_log(block) {
            let wy = base_y + vy;
            trunk_bottom
                .entry((wx, wz))
                .and_modify(|e| {
                    if wy < e.0 {
                        *e = (wy, block);
                    }
                })
                .or_insert((wy, block));
        }
    }
    // Root pass: anchor genuine ground-rooted trunk columns straight down to their own local
    // ground so off-center trunks / buttress legs / prop-roots on a slope do not float. Branch
    // tips are skipped. The range is empty on flat/uphill ground.
    let min_log_vy = trunk_bottom
        .values()
        .map(|(top, _)| top - base_y)
        .min()
        .unwrap_or(0);
    for ((wx, wz), (top, log)) in trunk_bottom {
        if top - base_y > min_log_vy + ROOT_BASE_VY {
            continue;
        }
        if editor.is_lc_water(wx, wz) {
            continue;
        }
        let gy = editor.get_absolute_y(wx, y_offset, wz);
        let from = (top - 1 - ROOT_MAX).max(gy);
        let to = top - 1;
        for wy in from..=to {
            editor.set_block_absolute(log, wx, wy, wz, None, Some(blacklist));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_corners_and_bounds() {
        let (w, l) = (3, 5);
        assert_eq!(rotate_xz(0, 0, w, l, 0), (0, 0));
        assert_eq!(rotate_xz(0, 0, w, l, 2), (w - 1, l - 1));
        for x in 0..w {
            for z in 0..l {
                let (rx, rz) = rotate_xz(x, z, w, l, 1);
                assert!((0..l).contains(&rx) && (0..w).contains(&rz));
                let (rx3, rz3) = rotate_xz(x, z, w, l, 3);
                assert!((0..l).contains(&rx3) && (0..w).contains(&rz3));
            }
        }
    }

    #[test]
    fn varint_decode() {
        assert_eq!(decode_varints(&[0x00]), vec![0]);
        assert_eq!(decode_varints(&[0x7F]), vec![127]);
        assert_eq!(decode_varints(&[0x80, 0x01]), vec![128]);
        assert_eq!(decode_varints(&[0xAC, 0x02]), vec![300]);
        assert_eq!(decode_varints(&[0x01, 0x80, 0x01]), vec![1, 128]);
    }

    #[test]
    fn block_mapping_keeps_logs_and_leaves() {
        assert!(map_block("minecraft:oak_log[axis=y]").is_some());
        assert!(map_block("minecraft:spruce_leaves[distance=7,persistent=false]").is_some());
        assert!(map_block("oak_wood").is_some());
        assert!(map_block("minecraft:flowering_azalea_leaves").is_some());
        assert!(map_block("minecraft:stripped_jungle_log").is_some());
        assert!(map_block("minecraft:pale_oak_log").is_some());
        assert!(map_block("minecraft:mangrove_roots").is_some());
        assert!(map_block("minecraft:bamboo_block").is_some());
        assert!(map_block("minecraft:vine").is_some()); // canopy foliage, kept as leaves
    }

    #[test]
    fn block_mapping_drops_unwanted() {
        assert!(map_block("minecraft:air").is_none());
        assert!(map_block("minecraft:creaking_heart").is_none());
        assert!(map_block("minecraft:short_grass").is_none());
        assert!(map_block("minecraft:cocoa[age=2]").is_none());
    }

    #[test]
    fn trunk_slots_stay_at_least_one_block_apart() {
        let mut slots = std::collections::HashSet::new();
        for x in -30..30 {
            for z in -30..30 {
                slots.insert(trunk_slot_s(x, z, 3));
            }
        }
        let v: Vec<(i32, i32)> = slots.into_iter().collect();
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let (dx, dz) = ((v[i].0 - v[j].0).abs(), (v[i].1 - v[j].1).abs());
                assert!(dx.max(dz) >= 2, "slots {:?} and {:?} touch", v[i], v[j]);
            }
        }
    }
}
