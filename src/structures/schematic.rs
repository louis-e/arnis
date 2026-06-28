//! Generic Sponge .schem loader keeping full blocks + states, for stamping props.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use fastnbt::Value;

use crate::block_definitions::*;
use crate::trees::schematic::rotate_xz;
use crate::world_editor::WorldEditor;

/// Parsed structure: voxels with block-states, plus the tallest column as anchor.
pub struct StructureSchematic {
    pub width: i32,
    pub length: i32,
    pub voxels: Vec<(i32, i32, i32, BlockWithProperties)>,
    pub anchor_x: i32,
    pub anchor_z: i32,
}

/// Map a Minecraft block-state string to a block + parsed properties; None for air/unmodeled.
fn map_structure_block(name: &str) -> Option<BlockWithProperties> {
    let base = name
        .split('[')
        .next()
        .unwrap_or(name)
        .trim_start_matches("minecraft:");
    let block = match base {
        "sandstone" => SANDSTONE,
        "smooth_sandstone" => SMOOTH_SANDSTONE,
        "smooth_sandstone_stairs" => SMOOTH_SANDSTONE_STAIRS,
        "cut_sandstone_slab" => CUT_SANDSTONE_SLAB,
        "sandstone_wall" => SANDSTONE_WALL,
        "andesite" => ANDESITE,
        "andesite_wall" => ANDESITE_WALL,
        "diorite_wall" => DIORITE_WALL,
        "stone" => STONE,
        "stone_slab" => STONE_BLOCK_SLAB,
        "smooth_quartz_slab" => SMOOTH_QUARTZ_SLAB,
        "smooth_quartz_stairs" => SMOOTH_QUARTZ_STAIRS,
        "blackstone_stairs" => BLACKSTONE_STAIRS,
        "blackstone_wall" => BLACKSTONE_WALL,
        "iron_bars" => IRON_BARS,
        // Chain axis is carried through properties; either const serialises to "chain".
        "iron_chain" | "chain" => CHAIN_X,
        "glass" => GLASS,
        "ladder" => LADDER,
        "lever" => LEVER,
        "scaffolding" => SCAFFOLDING,
        "white_carpet" => WHITE_CARPET,
        "anvil" | "chipped_anvil" => ANVIL,
        "black_wall_banner" => BLACK_WALL_BANNER,
        "birch_trapdoor" => BIRCH_TRAPDOOR,
        "dark_oak_trapdoor" => DARK_OAK_TRAPDOOR,
        "iron_trapdoor" => IRON_TRAPDOOR,
        "jungle_trapdoor" => JUNGLE_TRAPDOOR,
        "birch_fence" => BIRCH_FENCE,
        "jungle_fence" => JUNGLE_FENCE,
        "birch_fence_gate" => BIRCH_FENCE_GATE,
        "dark_oak_fence_gate" => DARK_OAK_FENCE_GATE,
        "birch_door" => BIRCH_DOOR,
        "birch_pressure_plate" => BIRCH_PRESSURE_PLATE,
        "stone_pressure_plate" => STONE_PRESSURE_PLATE,
        "blast_furnace" => BLAST_FURNACE,
        "dispenser" => DISPENSER,
        "hopper" => HOPPER,
        "grindstone" => GRINDSTONE,
        "lantern" => LANTERN,
        "lodestone" => LODESTONE,
        "redstone_torch" => REDSTONE_TORCH,
        // Excavator blocks.
        "grass" => GRASS,
        "oak_log" => OAK_LOG,
        "stone_brick_wall" => STONE_BRICK_WALL,
        "mossy_stone_brick_wall" => MOSSY_STONE_BRICK_WALL,
        "end_stone_brick_wall" => END_STONE_BRICK_WALL,
        "polished_deepslate_wall" => POLISHED_DEEPSLATE_WALL,
        "polished_deepslate_slab" => POLISHED_DEEPSLATE_SLAB,
        "smooth_stone_slab" => SMOOTH_STONE_SLAB,
        "polished_andesite_slab" => POLISHED_ANDESITE_SLAB,
        "polished_andesite_stairs" => POLISHED_ANDESITE_STAIRS,
        "bamboo_stairs" => BAMBOO_STAIRS,
        "bamboo_slab" => BAMBOO_SLAB,
        "yellow_terracotta" => YELLOW_TERRACOTTA,
        "black_stained_glass" => BLACK_STAINED_GLASS,
        "chiseled_polished_blackstone" => CHISELED_POLISHED_BLACKSTONE,
        "chiseled_deepslate" => CHISELED_DEEPSLATE,
        "stone_button" => STONE_BUTTON,
        "lightning_rod" => LIGHTNING_ROD,
        // Playground blocks.
        "short_grass" => GRASS,
        "chest" => CHEST,
        "glowstone" => GLOWSTONE,
        "spruce_log" => SPRUCE_LOG,
        "dark_oak_log" => DARK_OAK_LOG,
        "dark_oak_planks" => DARK_OAK_PLANKS,
        "cobblestone_stairs" => COBBLESTONE_STAIRS,
        "cobblestone_slab" => COBBLESTONE_SLAB,
        "jungle_stairs" => JUNGLE_STAIRS,
        "jungle_slab" => JUNGLE_SLAB,
        "dark_oak_slab" => DARK_OAK_SLAB,
        "spruce_slab" => SPRUCE_SLAB,
        "oak_slab" => OAK_SLAB,
        "oak_fence" => OAK_FENCE,
        "spruce_fence" => SPRUCE_FENCE,
        "nether_brick_fence" => NETHER_BRICK_FENCE,
        "oak_trapdoor" => OAK_TRAPDOOR,
        "oak_button" => OAK_BUTTON,
        "birch_button" => BIRCH_BUTTON,
        "powered_rail" => POWERED_RAIL,
        // Boat + tractor blocks. Block-entity/invisible blocks (barriers, skulls, signs) are dropped.
        "andesite_slab" => ANDESITE_SLAB,
        "cobbled_deepslate_slab" => COBBLED_DEEPSLATE_SLAB,
        "cobbled_deepslate_stairs" => COBBLED_DEEPSLATE_STAIRS,
        "cyan_terracotta" => CYAN_TERRACOTTA,
        "dark_oak_fence" => DARK_OAK_FENCE,
        "dark_oak_stairs" => DARK_OAK_STAIRS,
        "dark_oak_pressure_plate" => DARK_OAK_PRESSURE_PLATE,
        "oak_pressure_plate" => OAK_PRESSURE_PLATE,
        "oak_fence_gate" => OAK_FENCE_GATE,
        "spruce_stairs" => SPRUCE_STAIRS,
        "spruce_trapdoor" => SPRUCE_TRAPDOOR,
        "spruce_button" => SPRUCE_BUTTON,
        "spruce_fence_gate" => SPRUCE_FENCE_GATE,
        "end_rod" => END_ROD,
        "flower_pot" => FLOWER_POT,
        "sea_pickle" => SEA_PICKLE,
        "gray_concrete_powder" => GRAY_CONCRETE_POWDER,
        "gray_stained_glass_pane" => GRAY_STAINED_GLASS_PANE,
        "gray_wall_banner" => GRAY_WALL_BANNER,
        "gray_wool" => GRAY_WOOL,
        "nether_wart_block" => NETHER_WART_BLOCK,
        "polished_basalt" => POLISHED_BASALT,
        "polished_blackstone_button" => POLISHED_BLACKSTONE_BUTTON,
        "polished_blackstone_pressure_plate" => POLISHED_BLACKSTONE_PRESSURE_PLATE,
        "red_nether_brick_slab" => RED_NETHER_BRICK_SLAB,
        "red_nether_brick_stairs" => RED_NETHER_BRICK_STAIRS,
        "stone_brick_slab" => STONE_BRICK_SLAB,
        // Lighthouse blocks (vanilla; modded furniture is dropped by the no-match arm).
        "stone_bricks" => STONE_BRICKS,
        "cracked_stone_bricks" => CRACKED_STONE_BRICKS,
        "mossy_stone_bricks" => MOSSY_STONE_BRICKS,
        "stone_brick_stairs" => STONE_BRICK_STAIRS,
        "end_stone_bricks" => END_STONE_BRICKS,
        "end_stone_brick_slab" => END_STONE_BRICK_SLAB,
        "smooth_red_sandstone" => SMOOTH_RED_SANDSTONE,
        "smooth_red_sandstone_slab" => SMOOTH_RED_SANDSTONE_SLAB,
        "nether_brick_stairs" => NETHER_BRICK_STAIRS,
        "nether_brick_wall" => NETHER_BRICK_WALL,
        "glass_pane" => GLASS_PANE,
        "spruce_planks" => SPRUCE_PLANKS,
        "oak_door" => OAK_DOOR,
        "acacia_trapdoor" => ACACIA_TRAPDOOR,
        "dark_oak_button" => DARK_OAK_BUTTON,
        "barrel" => BARREL,
        "composter" => COMPOSTER,
        "smoker" => SMOKER,
        "crafting_table" => CRAFTING_TABLE,
        "cyan_carpet" => CYAN_CARPET,
        "green_carpet" => GREEN_CARPET,
        "light_blue_carpet" => LIGHT_BLUE_CARPET,
        "potted_oxeye_daisy" => FLOWER_POT,
        "tall_grass" => GRASS,
        // Fountain blocks. Water makes the basin; player_head (block entity) is dropped.
        "water" => WATER,
        "cobblestone" => COBBLESTONE,
        "cobblestone_wall" => COBBLESTONE_WALL,
        "chiseled_stone_bricks" => CHISELED_STONE_BRICKS,
        "stone_stairs" => STONE_STAIRS,
        "coarse_dirt" => COARSE_DIRT,
        "mossy_cobblestone" => MOSSY_COBBLESTONE,
        "mossy_cobblestone_slab" => MOSSY_COBBLESTONE_SLAB,
        "mossy_stone_brick_slab" => MOSSY_STONE_BRICK_SLAB,
        "polished_andesite" => POLISHED_ANDESITE,
        "light_gray_concrete" => LIGHT_GRAY_CONCRETE,
        "light_gray_carpet" => LIGHT_GRAY_CARPET,
        "cyan_wool" => CYAN_WOOL,
        "prismarine" => PRISMARINE,
        "blue_stained_glass_pane" => BLUE_STAINED_GLASS_PANE,
        "tripwire_hook" => TRIPWIRE_HOOK,
        "oak_leaves" => OAK_LEAVES,
        "poppy" => RED_FLOWER,
        "red_tulip" => RED_FLOWER,
        "dandelion" => YELLOW_FLOWER,
        "potted_azalea_bush" | "potted_flowering_azalea_bush" => FLOWER_POT,
        _ => return None,
    };
    Some(BlockWithProperties::new(block, parse_state(name)))
}

/// Parse the `[k=v,...]` suffix into an NBT compound (values as strings); None if no state.
fn parse_state(name: &str) -> Option<Value> {
    let start = name.find('[')?;
    let end = name.rfind(']')?;
    if end <= start + 1 {
        return None;
    }
    let mut props: HashMap<String, Value> = HashMap::new();
    for kv in name[start + 1..end].split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            props.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
        }
    }
    if props.is_empty() {
        None
    } else {
        Some(Value::Compound(props))
    }
}

/// Decode a Sponge `Data`/`BlockData` byte stream: LEB128 varint palette indices.
fn decode_varints(data: &[u8]) -> Vec<i32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let mut val: i32 = 0;
        let mut shift = 0u32;
        loop {
            let byte = data[i];
            i += 1;
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

/// Load a gzipped Sponge `.schem` (v2 or v3) keeping all mapped blocks + states.
pub fn load_structure(gz_bytes: &[u8]) -> Result<StructureSchematic, String> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz_bytes)
        .read_to_end(&mut raw)
        .map_err(|e| format!("schem: gunzip failed: {e}"))?;
    let root: Value = fastnbt::from_bytes(&raw).map_err(|e| format!("schem: nbt parse: {e}"))?;
    let root_c = as_compound(&root).ok_or("schem: root not a compound")?;
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

    let (palette_v, data_v) = match scm.get("Blocks").and_then(as_compound) {
        Some(blocks) => (blocks.get("Palette"), blocks.get("Data")),
        None => (scm.get("Palette"), scm.get("BlockData")),
    };
    let palette = palette_v
        .and_then(as_compound)
        .ok_or("schem: missing Palette")?;

    let mut idx_to_block: HashMap<i32, BlockWithProperties> = HashMap::new();
    for (name, v) in palette {
        if let Value::Int(i) = v {
            if let Some(bwp) = map_structure_block(name) {
                idx_to_block.insert(*i, bwp);
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
        if let Some(bwp) = idx_to_block.get(&idx) {
            let x = i % width;
            let z = (i / width) % length;
            let y = i / wl;
            voxels.push((x, y, z, bwp.clone()));
        }
    }

    // Drop empty layers below so the model's lowest block sits at y=0.
    if let Some(min_y) = voxels.iter().map(|v| v.1).min() {
        if min_y != 0 {
            for v in &mut voxels {
                v.1 -= min_y;
            }
        }
    }
    voxels.shrink_to_fit();

    // Anchor on the tallest column (the mast) so callers can place it precisely.
    let mut anchor = (0, 0);
    let mut best = -1;
    for &(x, y, z, _) in &voxels {
        if y > best {
            best = y;
            anchor = (x, z);
        }
    }

    Ok(StructureSchematic {
        width,
        length,
        voxels,
        anchor_x: anchor.0,
        anchor_z: anchor.1,
    })
}

/// Rotate a cardinal direction string by `k` clockwise quarter-turns.
fn rotate_dir(d: &str, k: u8) -> &'static str {
    const ORDER: [&str; 4] = ["north", "east", "south", "west"];
    match ORDER.iter().position(|&x| x == d) {
        Some(i) => ORDER[(i + k as usize) & 3],
        None => match d {
            "up" => "up",
            "down" => "down",
            _ => "north",
        },
    }
}

/// Rotate a rail `shape` by `k` quarter-turns; stairs shapes carry no absolute dir, returned as-is.
fn rotate_rail_shape(s: &str, k: u8) -> String {
    match s {
        "north_south" | "east_west" => if k & 1 == 1 {
            if s == "north_south" {
                "east_west"
            } else {
                "north_south"
            }
        } else {
            s
        }
        .to_string(),
        _ if s.starts_with("ascending_") => format!("ascending_{}", rotate_dir(&s[10..], k)),
        "north_east" | "north_west" | "south_east" | "south_west" => {
            let mut it = s.split('_');
            let a = rotate_dir(it.next().unwrap_or("north"), k);
            let b = rotate_dir(it.next().unwrap_or("east"), k);
            let (ns, ew) = if a == "north" || a == "south" {
                (a, b)
            } else {
                (b, a)
            };
            format!("{ns}_{ew}")
        }
        _ => s.to_string(),
    }
}

/// Rotate a block-state compound by `k` quarter-turns: facing, connection sides, rail shape, axis.
fn rotate_props(props: &Value, k: u8) -> Value {
    let Value::Compound(c) = props else {
        return props.clone();
    };
    let mut out: HashMap<String, Value> = HashMap::with_capacity(c.len());
    for (key, val) in c {
        match (key.as_str(), val) {
            ("north" | "south" | "east" | "west", _) => {
                out.insert(rotate_dir(key, k).to_string(), val.clone());
            }
            ("facing", Value::String(s)) => {
                out.insert("facing".into(), Value::String(rotate_dir(s, k).to_string()));
            }
            ("shape", Value::String(s)) => {
                out.insert("shape".into(), Value::String(rotate_rail_shape(s, k)));
            }
            ("axis", Value::String(s)) if k & 1 == 1 => {
                let a = match s.as_str() {
                    "x" => "z",
                    "z" => "x",
                    o => o,
                };
                out.insert("axis".into(), Value::String(a.to_string()));
            }
            _ => {
                out.insert(key.clone(), val.clone());
            }
        }
    }
    Value::Compound(out)
}

/// Stamp anchor at (base_x, base_z), lowest voxel at base_y, rotated by `rot`; `ground` fills under each column. Keep half-extent under TILE_EDITOR_HALO (64) or it clips at tile seams.
pub fn place_structure(
    editor: &mut WorldEditor,
    schem: &StructureSchematic,
    base_x: i32,
    base_z: i32,
    base_y: i32,
    rot: u8,
    ground: Option<Block>,
) {
    let k = rot & 3;
    let (w, l) = (schem.width, schem.length);
    debug_assert!(
        w.max(l) <= 64,
        "structure exceeds tile halo; clips at seams"
    );
    let (ax, az) = rotate_xz(schem.anchor_x, schem.anchor_z, w, l, k);
    for (vx, vy, vz, bwp) in &schem.voxels {
        let (rx, rz) = rotate_xz(*vx, *vz, w, l, k);
        let wx = base_x + rx - ax;
        let wz = base_z + rz - az;
        let placed = if k == 0 {
            bwp.clone()
        } else {
            let props = bwp
                .properties
                .as_ref()
                .map(|p| Arc::new(rotate_props(p, k)));
            BlockWithProperties::from_arc(bwp.block, props)
        };
        editor.set_block_with_properties_absolute(placed, wx, base_y + vy, wz, None, None);
        if let Some(g) = ground {
            editor.set_block_absolute(g, wx, base_y - 1, wz, None, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_decode() {
        assert_eq!(decode_varints(&[0xAC, 0x02]), vec![300]);
        assert_eq!(decode_varints(&[0x01, 0x80, 0x01]), vec![1, 128]);
    }

    #[test]
    fn parse_state_reads_properties() {
        let v = parse_state("minecraft:sandstone_wall[east=low,up=true]").unwrap();
        match v {
            Value::Compound(c) => {
                assert_eq!(c.get("east"), Some(&Value::String("low".to_string())));
                assert_eq!(c.get("up"), Some(&Value::String("true".to_string())));
            }
            _ => panic!("expected compound"),
        }
        assert!(parse_state("minecraft:stone").is_none());
    }

    #[test]
    fn maps_known_blocks_drops_unknown() {
        assert!(map_structure_block("minecraft:sandstone").is_some());
        assert!(map_structure_block("minecraft:iron_bars[north=true]").is_some());
        assert!(map_structure_block("minecraft:air").is_none());
        assert!(map_structure_block("minecraft:diamond_block").is_none());
    }
}
