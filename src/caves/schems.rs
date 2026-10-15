//! Cave asset-pack engine — stamps curated Sponge `.schem` formations (ice spikes, snow piles,
//! dripstone columns, amethyst clusters, dripleaf, fluid pockets, glow-lichen patches…) into the
//! carved cave network, the underground counterpart of the surface tree schematics. Differences vs
//! the tree loader (src/trees/schematic.rs): blocks keep their FULL block-states (pointed_dripstone
//! tip/frustum, bud facing, snow layers…), anchoring is cave-floor/cave-ceiling instead of surface
//! ground level, and solid voxels are written with an AIR whitelist so a formation poking into a wall
//! is clipped for free (never cuts rock/ore/fluid).
//!
//! Pack layout: a directory of `.schem` files plus a
//! `cave_pack.json` listing families `{name, orientation: up|down|cover|embed|embed_floor|any,
//! biomes: [zone tags], weight, files, sink?}`. Resolution order: `--cave-asset-pack <DIR>` flag,
//! else a `cave-pack/` directory next to the running exe, else the pass no-ops (caves still get
//! their procedural decoration).

use super::decoration::{Decor, Zone, ROCK};
use super::rng::XoroRandom;
use crate::block_definitions::*;
use crate::trees::schematic::rotate_xz;
use crate::world_editor::{terrain_floor_y, WorldEditor};
use fastnbt::Value;
use serde::Deserialize;
// FnvHashSet: std seeds its hasher randomly per process, so iterating one to
// apply world edits gives a different write order -- and a different world --
// every run. FNV is deterministic. See the note in caves/mod.rs.
use fnv::{FnvHashMap, FnvHashSet as HashSet};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[derive(Deserialize)]
struct MFamily {
    #[allow(dead_code)] // kept for debugging dumps
    name: String,
    orientation: String,
    biomes: Vec<String>,
    weight: u32,
    files: Vec<String>,
    /// blocks to SINK the formation into the floor (base carves into rock) — anchors chunky
    /// formations (big amethyst clusters, ice spikes) so they grow OUT of the ground/wall instead
    /// of balancing on one voxel and reading as floating.
    #[serde(default)]
    sink: i32,
}
#[derive(Deserialize)]
struct MPack {
    families: Vec<MFamily>,
}

#[derive(Clone, Copy, PartialEq)]
enum Orient {
    Up,
    Down,
    Cover,
    Embed,
    EmbedFloor,
    Any,
}

/// One palette entry: the mapped block + its block-state pairs (kept as strings so directional
/// states can be rotated at placement time).
struct PalEntry {
    block: Block,
    props: Vec<(String, String)>,
    is_fluid: bool,
    is_air: bool,
}

pub struct CaveSchem {
    w: i32,
    h: i32,
    l: i32,
    /// (dx, dy, dz, palette index) — includes explicit AIR voxels (needed by embed families that
    /// must dig their pocket); up/down/cover placement skips them.
    voxels: Vec<(i32, i32, i32, u16)>,
    palette: Vec<PalEntry>,
    solid_count: usize,
}

struct Family {
    orientation: Orient,
    biomes: Vec<String>,
    weight: u32,
    sink: i32,
    schems: Vec<CaveSchem>,
}

pub struct CavePack {
    families: Vec<Family>,
}

static PACK: OnceLock<Option<CavePack>> = OnceLock::new();

/// Resolve + load the pack once per process. Explicit dir wins; otherwise `cave-pack/` next to the
/// exe; otherwise the stamping pass is disabled.
pub fn init_pack(explicit: Option<&Path>) {
    PACK.get_or_init(|| {
        let dir: Option<PathBuf> = match explicit {
            Some(d) => Some(d.to_path_buf()),
            None => std::env::current_exe().ok().and_then(|e| {
                let d = e.parent()?.join("cave-pack");
                if d.join("cave_pack.json").is_file() {
                    Some(d)
                } else {
                    None
                }
            }),
        };
        let dir = dir?;
        match load_pack(&dir) {
            Ok(p) => {
                let n: usize = p.families.iter().map(|f| f.schems.len()).sum();
                println!(
                    "Loaded cave asset pack: {} schematics in {} families",
                    n,
                    p.families.len()
                );
                Some(p)
            }
            Err(e) => {
                eprintln!(
                    "Warning: cave asset pack at {} failed to load: {e}",
                    dir.display()
                );
                None
            }
        }
    });
}

fn load_pack(dir: &Path) -> Result<CavePack, String> {
    let manifest = std::fs::read_to_string(dir.join("cave_pack.json"))
        .map_err(|e| format!("cave_pack.json: {e}"))?;
    let m: MPack = serde_json::from_str(&manifest).map_err(|e| format!("cave_pack.json: {e}"))?;
    let mut families = Vec::new();
    for f in m.families {
        let orientation = match f.orientation.as_str() {
            "up" => Orient::Up,
            "down" => Orient::Down,
            "cover" => Orient::Cover,
            "embed" => Orient::Embed,
            "embed_floor" => Orient::EmbedFloor,
            _ => Orient::Any,
        };
        let mut schems = Vec::new();
        // covers smaller than 3 solid blocks read as one random floating cube — drop them.
        let min_solid = if orientation == Orient::Cover { 3 } else { 1 };
        for file in &f.files {
            let bytes = std::fs::read(dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
            match load_cave_schem(&bytes) {
                Ok(s) if s.solid_count >= min_solid => schems.push(s),
                Ok(_) => {}
                Err(e) => eprintln!("Warning: cave schem {file}: {e}"),
            }
        }
        if !schems.is_empty() {
            families.push(Family {
                orientation,
                biomes: f.biomes,
                weight: f.weight.max(1),
                sink: f.sink.max(0),
                schems,
            });
        }
    }
    Ok(CavePack { families })
}

// ---- Sponge .schem parsing (v2/v3), keeping block states ----

fn as_compound(v: &Value) -> Option<&HashMap<String, Value>> {
    match v {
        Value::Compound(c) => Some(c),
        _ => None,
    }
}
/// A schematic dimension: Sponge stores them as unsigned shorts, and every one must be positive.
fn dimension(c: &HashMap<String, Value>, k: &str) -> Result<i32, String> {
    let v = match c.get(k) {
        Some(Value::Short(s)) => i32::from(*s as u16),
        Some(Value::Int(i)) => *i,
        _ => return Err(format!("missing field {k}")),
    };
    if v <= 0 {
        return Err(format!("{k} must be positive, got {v}"));
    }
    Ok(v)
}

/// Decode Sponge's varint block data. A value can take at most five bytes (an i32); a longer
/// one would overflow the shift.
fn decode_varints(bytes: &[u8]) -> Result<Vec<i32>, String> {
    let mut out = Vec::new();
    let mut cur: u32 = 0;
    let mut shift = 0;
    for &b in bytes {
        if shift > 28 {
            return Err("BlockData holds a varint longer than 5 bytes".into());
        }
        cur |= u32::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            out.push(cur as i32);
            cur = 0;
            shift = 0;
        } else {
            shift += 7;
        }
    }
    if shift != 0 {
        return Err("BlockData ends inside a varint".into());
    }
    Ok(out)
}

/// Map a palette base name (no `minecraft:`, no states) to our Block. `None` = voxel dropped
/// (unknown block, or the pack's acacia-leaf marker). AIR maps to a real entry (embed needs it).
fn cave_map(base: &str, props: &[(String, String)]) -> Option<(Block, bool)> {
    // (block, is_air)
    let b = match base {
        "air" => return Some((AIR, true)),
        "acacia_leaves" => return None, // pack author's placeholder/marker block
        "ice" => ICE,
        "packed_ice" => PACKED_ICE,
        "blue_ice" => BLUE_ICE,
        "snow_block" => SNOW_BLOCK,
        "snow" => SNOW_LAYER,
        "powder_snow" => POWDER_SNOW,
        "cobweb" => COBWEB,
        "glow_lichen" => GLOW_LICHEN,
        "amethyst_block" => AMETHYST_BLOCK,
        "budding_amethyst" => BUDDING_AMETHYST,
        "amethyst_cluster" => AMETHYST_CLUSTER,
        "small_amethyst_bud" => SMALL_AMETHYST_BUD,
        "medium_amethyst_bud" => MEDIUM_AMETHYST_BUD,
        "large_amethyst_bud" => LARGE_AMETHYST_BUD,
        "dripstone_block" => DRIPSTONE_BLOCK,
        "pointed_dripstone" => POINTED_DRIPSTONE,
        "water" => WATER,
        "lava" => LAVA,
        "clay" => CLAY,
        "big_dripleaf" => BIG_DRIPLEAF,
        "big_dripleaf_stem" => BIG_DRIPLEAF_STEM,
        "small_dripleaf" => {
            if props.iter().any(|(k, v)| k == "half" && v == "upper") {
                SMALL_DRIPLEAF_UPPER
            } else {
                SMALL_DRIPLEAF_LOWER
            }
        }
        _ => return None,
    };
    Some((b, false))
}

fn load_cave_schem(gz_bytes: &[u8]) -> Result<CaveSchem, String> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz_bytes)
        .read_to_end(&mut raw)
        .map_err(|e| format!("gunzip: {e}"))?;
    let root: Value = fastnbt::from_bytes(&raw).map_err(|e| format!("nbt: {e}"))?;
    let root_c = as_compound(&root).ok_or("root not compound")?;
    let scm = root_c
        .get("Schematic")
        .and_then(as_compound)
        .unwrap_or(root_c);

    let w = dimension(scm, "Width")?;
    let nominal_h = dimension(scm, "Height")?; // recomputed from non-air voxels after normalize
    let l = dimension(scm, "Length")?;
    let (palette_v, data_v) = match scm.get("Blocks").and_then(as_compound) {
        Some(blocks) => (blocks.get("Palette"), blocks.get("Data")),
        None => (scm.get("Palette"), scm.get("BlockData")),
    };
    let palette_c = palette_v.and_then(as_compound).ok_or("missing Palette")?;

    // palette: schem index -> our entry index (usize::MAX = dropped). Sponge numbers the entries
    // 0..len, so anything outside that is a corrupt file, refused before it can index anything.
    let entries = palette_c.len();
    if entries > usize::from(u16::MAX) {
        return Err(format!(
            "palette has {entries} entries, more than supported"
        ));
    }
    let mut remap = vec![usize::MAX; entries];
    let mut palette: Vec<PalEntry> = Vec::new();
    for (name, v) in palette_c {
        let Value::Int(i) = v else { continue };
        let slot = usize::try_from(*i)
            .ok()
            .filter(|&slot| slot < entries)
            .ok_or_else(|| format!("palette index {i} is outside 0..{entries}"))?;
        let (base, props) = match name.split_once('[') {
            Some((b, rest)) => {
                let rest = rest.trim_end_matches(']');
                let pairs: Vec<(String, String)> = rest
                    .split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    // waterlogged=false is the default for everything we place; fluids carry no
                    // usable states (water[level=0] = a plain source block).
                    .filter(|(k, v)| {
                        !(k == "waterlogged" && v == "false")
                            && k != "level"
                            && k != "distance"
                            && k != "persistent"
                    })
                    .collect();
                (b.trim_start_matches("minecraft:"), pairs)
            }
            None => (name.trim_start_matches("minecraft:"), Vec::new()),
        };
        if let Some((block, is_air)) = cave_map(base, &props) {
            let is_fluid = block == WATER || block == LAVA;
            remap[slot] = palette.len();
            palette.push(PalEntry {
                block,
                props,
                is_fluid,
                is_air,
            });
        }
    }

    let data_bytes: Vec<u8> = match data_v {
        Some(Value::ByteArray(b)) => b.iter().map(|&x| x as u8).collect(),
        _ => return Err("missing BlockData".into()),
    };
    let indices = decode_varints(&data_bytes)?;

    // Sponge requires exactly Width*Height*Length entries. This loader reads a user-droppable
    // cave-pack directory, so it is the one that actually faces untrusted files: a stream that
    // runs long or short folds into out-of-range coordinates instead of failing.
    let volume = i64::from(w) * i64::from(nominal_h) * i64::from(l);
    if volume > i64::from(i32::MAX) {
        return Err("volume exceeds the supported range".into());
    }
    if indices.len() as i64 != volume {
        return Err(format!(
            "BlockData has {} entries, expected {volume}",
            indices.len()
        ));
    }

    let wl = w * l;
    let mut voxels = Vec::new();
    let mut solid_count = 0usize;
    for (i, &idx) in indices.iter().enumerate() {
        let pi = *remap.get(idx as usize).unwrap_or(&usize::MAX);
        if pi == usize::MAX {
            continue;
        }
        let i = i as i32;
        let (x, z, y) = (i % w, (i / w) % l, i / wl);
        if !palette[pi].is_air {
            solid_count += 1;
        }
        voxels.push((x, y, z, pi as u16));
    }
    // normalize so the lowest NON-AIR voxel sits at dy=0 (schems often pad below).
    if let Some(min_y) = voxels
        .iter()
        .filter(|&&(_, _, _, pi)| !palette[pi as usize].is_air)
        .map(|v| v.1)
        .min()
    {
        if min_y != 0 {
            for v in &mut voxels {
                v.1 -= min_y;
            }
            voxels.retain(|v| v.1 >= 0);
        }
    }
    let h = voxels
        .iter()
        .filter(|&&(_, _, _, pi)| !palette[pi as usize].is_air)
        .map(|v| v.1)
        .max()
        .unwrap_or(0)
        + 1;
    // Water and lava may never touch. Placement checks the fluids around a formation, but one
    // that brings both, side by side, would break that wherever it lands, so it is refused here.
    let fluids: FnvHashMap<(i32, i32, i32), Block> = voxels
        .iter()
        .filter(|&&(_, _, _, pi)| palette[pi as usize].is_fluid)
        .map(|&(x, y, z, pi)| ((x, y, z), palette[pi as usize].block))
        .collect();
    let lava_beside = |x: i32, y: i32, z: i32| {
        super::neighbours(x, y, z)
            .iter()
            .any(|n| fluids.get(n) == Some(&LAVA))
    };
    if fluids
        .iter()
        .any(|(&(x, y, z), &b)| b == WATER && lava_beside(x, y, z))
    {
        return Err("water touches lava".into());
    }
    Ok(CaveSchem {
        w,
        h,
        l,
        voxels,
        palette,
        solid_count,
    })
}

// ---- placement ----

#[inline]
fn chunk_rng(seed: i64, cx: i32, cz: i32, salt: i64) -> XoroRandom {
    let s = seed
        ^ (cx as i64).wrapping_mul(341873128712)
        ^ (cz as i64).wrapping_mul(132897987541)
        ^ salt;
    XoroRandom::from_seed(s)
}

/// rotate a direction string k quarter-turns clockwise.
fn rot_dir(d: &str, k: u8) -> &str {
    const RING: [&str; 4] = ["north", "east", "south", "west"];
    match RING.iter().position(|&x| x == d) {
        Some(i) => RING[(i + k as usize) % 4],
        None => d, // up/down/none
    }
}

/// rotate directional block-states by k quarter-turns: `facing=<dir>` values rotate; multiface
/// boolean KEYS (glow_lichen's north/east/south/west) rotate as keys.
fn rot_props(props: &[(String, String)], k: u8) -> Option<Arc<Value>> {
    if props.is_empty() {
        return None;
    }
    let mut m: HashMap<String, Value> = HashMap::new();
    for (key, val) in props {
        let (nk, nv) = if key == "facing" {
            (key.clone(), rot_dir(val, k).to_string())
        } else if ["north", "east", "south", "west"].contains(&key.as_str()) {
            (rot_dir(key, k).to_string(), val.clone())
        } else {
            (key.clone(), val.clone())
        };
        m.insert(nk, Value::String(nv));
    }
    Some(crate::block_definitions::intern_props(Value::Compound(m)))
}

/// Stamp cave formations across the tile. Runs AFTER all carving + fluids (so clearance tests see
/// the final cave), BEFORE ores/decoration (so ore blobs don't spawn inside formations and the
/// AIR-only decoration naturally skips formation blocks). `air` is the final carved-air set.
#[allow(clippy::too_many_arguments)]
pub(super) fn stamp_region(
    editor: &mut WorldEditor,
    air: &HashSet<i64>,
    decor: &Decor,
    seed: i64,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    surf: &[i32],
    hh: usize,
    top_gate: i32,
    basin_fluid: &mut HashSet<i64>,
) {
    init_pack(None);
    let Some(pack) = PACK.get().and_then(|o| o.as_ref()) else {
        return;
    };
    let floor = terrain_floor_y();
    // rock + air: what embed placement may replace (never ore/fluid/buildings/bedrock).
    let rock_and_air: Vec<Block> = ROCK.iter().copied().chain([AIR]).collect();

    let cx0 = min_x.div_euclid(16);
    let cx1 = max_x.div_euclid(16);
    let cz0 = min_z.div_euclid(16);
    let cz1 = max_z.div_euclid(16);
    for cx in cx0..=cx1 {
        for cz in cz0..=cz1 {
            let mut r = chunk_rng(seed, cx, cz, 0x5CE3_5CE3);
            let mut placed = 0;
            for _try in 0..6 {
                if placed >= 2 {
                    break;
                }
                let gx = cx * 16 + r.next_int(16);
                let gz = cz * 16 + r.next_int(16);
                if gx < min_x || gx > max_x || gz < min_z || gz > max_z {
                    continue;
                }
                let surf_y = surf[(gx - min_x) as usize * hh + (gz - min_z) as usize];
                let top = surf_y - top_gate;
                // collect floor + ceiling anchors in this column from the air set.
                let mut floors: Vec<i32> = Vec::new();
                let mut ceils: Vec<i32> = Vec::new();
                for y in (floor + 6)..=top {
                    if !air.contains(&super::pack(gx, y, gz)) {
                        continue;
                    }
                    if !air.contains(&super::pack(gx, y - 1, gz))
                        && editor.check_for_block_absolute(gx, y - 1, gz, Some(ROCK), None)
                    {
                        floors.push(y);
                    }
                    if !air.contains(&super::pack(gx, y + 1, gz))
                        && y < top
                        && editor.check_for_block_absolute(gx, y + 1, gz, Some(ROCK), None)
                    {
                        ceils.push(y);
                    }
                }
                if floors.is_empty() && ceils.is_empty() {
                    continue;
                }
                // OPEN-SPACE GUARD: only stamp where the cave is at least 3x3 wide at the anchor —
                // a formation solidifying cells inside a 1-2-wide tunnel can sever the network's
                // connectors and fragment the cave system. Narrow passages stay clear.
                let is_open_3x3 = |y: i32| -> bool {
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            if !air.contains(&super::pack(gx + dx, y, gz + dz)) {
                                return false;
                            }
                        }
                    }
                    true
                };
                floors.retain(|&y| is_open_3x3(y));
                ceils.retain(|&y| is_open_3x3(y));
                if floors.is_empty() && ceils.is_empty() {
                    continue;
                }
                // zone tag at a representative anchor
                let probe_y = floors.first().or(ceils.first()).copied().unwrap();
                let zone = decor.zone(gx, probe_y, gz, surf_y);
                let tag = zone_tag(zone);
                // candidate families: matching tag; "any"-tagged families work everywhere (but only
                // 1-in-3 of the time in a themed zone, so themes keep their identity).
                let themed = r.next_int(3) != 0;
                let cands: Vec<usize> = pack
                    .families
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| {
                        if f.biomes.iter().any(|b| b == tag) {
                            true
                        } else {
                            f.biomes.iter().any(|b| b == "any") && (!themed || tag == "any")
                        }
                    })
                    .map(|(i, _)| i)
                    .collect();
                if cands.is_empty() {
                    continue;
                }
                let total_w: u32 = cands.iter().map(|&i| pack.families[i].weight).sum();
                let mut roll = r.next_int(total_w as i32) as u32;
                let mut fam = &pack.families[cands[0]];
                for &i in &cands {
                    let w = pack.families[i].weight;
                    if roll < w {
                        fam = &pack.families[i];
                        break;
                    }
                    roll -= w;
                }
                let schem = &fam.schems[r.next_int(fam.schems.len() as i32) as usize];
                let rot = r.next_int(4) as u8;

                let anchor_y = match fam.orientation {
                    Orient::Up | Orient::Any | Orient::EmbedFloor | Orient::Embed => {
                        if floors.is_empty() {
                            continue;
                        }
                        floors[r.next_int(floors.len() as i32) as usize]
                    }
                    Orient::Down => {
                        if ceils.is_empty() {
                            continue;
                        }
                        ceils[r.next_int(ceils.len() as i32) as usize]
                    }
                    Orient::Cover => {
                        // covers work both ways: floor-biased
                        if !floors.is_empty() && (ceils.is_empty() || r.next_int(10) < 7) {
                            floors[r.next_int(floors.len() as i32) as usize]
                        } else if !ceils.is_empty() {
                            ceils[r.next_int(ceils.len() as i32) as usize]
                        } else {
                            continue;
                        }
                    }
                };
                if place_schem(
                    editor,
                    air,
                    schem,
                    fam.orientation,
                    fam.sink,
                    gx,
                    anchor_y,
                    gz,
                    rot,
                    top,
                    min_x,
                    max_x,
                    min_z,
                    max_z,
                    surf,
                    hh,
                    top_gate,
                    &rock_and_air,
                    basin_fluid,
                ) {
                    placed += 1;
                }
            }
        }
    }
}

fn zone_tag(z: Zone) -> &'static str {
    match z {
        Zone::Lush => "lush",
        Zone::Dripstone => "dripstone",
        Zone::DeepDark => "deepdark",
        Zone::Mushroom => "mushroom",
        Zone::Ice => "ice",
        Zone::Amethyst => "amethyst",
        Zone::Volcanic => "volcanic",
        Zone::Normal => "any",
    }
}

/// Place one schematic. Returns false if the fit test failed (nothing written).
#[allow(clippy::too_many_arguments)]
fn place_schem(
    editor: &mut WorldEditor,
    air: &HashSet<i64>,
    schem: &CaveSchem,
    orient: Orient,
    sink: i32,
    ax: i32,
    ay: i32,
    az: i32,
    rot: u8,
    _col_top: i32,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    surf: &[i32],
    hh: usize,
    top_gate: i32,
    rock_and_air: &[Block],
    basin_fluid: &mut HashSet<i64>,
) -> bool {
    let floor = terrain_floor_y();
    let (fw, fl) = if rot % 2 == 1 {
        (schem.l, schem.w)
    } else {
        (schem.w, schem.l)
    };
    let cxo = (fw - 1) / 2;
    let czo = (fl - 1) / 2;
    let base_y = match orient {
        // sink: the formation's base carves a few blocks INTO the floor so it grows out of the
        // ground instead of balancing on one voxel and reading as a floating formation.
        Orient::Up | Orient::Any | Orient::Cover => ay - sink,
        Orient::Down => ay - (schem.h - 1),
        // embed: pocket's top row sits AT floor level (hidden powder-snow pit under a crust).
        Orient::Embed => ay - (schem.h - 1),
        // embed_floor (clay basins): FULLY sunk — top row replaces the floor block itself, so the
        // rim sits flush with the ground instead of standing on it as a raised ring.
        Orient::EmbedFloor => ay - schem.h,
    };
    // EmbedFloor GROUND CHECK: every solid voxel of the basin's bottom row must have real ground
    // under it — a basin anchored at a floor edge would otherwise hang over the void as a floating
    // basin.
    if orient == Orient::EmbedFloor {
        for &(vx, vy, vz, pi) in &schem.voxels {
            if vy != 0 || schem.palette[pi as usize].is_air {
                continue;
            }
            let (rx, rz) = rotate_xz(vx, vz, schem.w, schem.l, rot);
            let (wx, wz) = (ax + rx - cxo, az + rz - czo);
            if !editor.block_exists_absolute(wx, base_y - 1, wz) {
                return false;
            }
        }
    }

    // FLUID SAFETY: if any fluid voxel of this formation would sit 6-adjacent to the OPPOSITE fluid
    // already in the world (a pond next to the lava sea, a lava pocket beside a river), abort the
    // whole placement — a half-placed pond is worse than none, and water/lava contact is a standing
    // zero-tolerance invariant.
    for &(vx, vy, vz, pi) in &schem.voxels {
        let e = &schem.palette[pi as usize];
        if !e.is_fluid {
            continue;
        }
        let opposite = if e.block == WATER { LAVA } else { WATER };
        let (rx, rz) = rotate_xz(vx, vz, schem.w, schem.l, rot);
        let (wx, wy, wz) = (ax + rx - cxo, base_y + vy, az + rz - czo);
        for (nx, ny, nz) in [
            (wx + 1, wy, wz),
            (wx - 1, wy, wz),
            (wx, wy + 1, wz),
            (wx, wy - 1, wz),
            (wx, wy, wz + 1),
            (wx, wy, wz - 1),
        ] {
            if editor.check_for_block_absolute(nx, ny, nz, Some(&[opposite]), None) {
                return false;
            }
        }
    }

    // FIT TEST for up/down: enough of the formation must land in open cave air, or it reads as a
    // buried stump. Embed families skip this (they intentionally sit inside rock); the sunk rows of
    // a sink>0 formation are expected buried, so only rows at/above the anchor are scored.
    let into_rock = matches!(orient, Orient::Embed | Orient::EmbedFloor);
    if !into_rock {
        let (mut in_air, mut solid_total) = (0usize, 0usize);
        for &(vx, vy, vz, pi) in &schem.voxels {
            if schem.palette[pi as usize].is_air {
                continue;
            }
            let (rx, rz) = rotate_xz(vx, vz, schem.w, schem.l, rot);
            let (wx, wy, wz) = (ax + rx - cxo, base_y + vy, az + rz - czo);
            if wy < ay {
                continue; // sunk rows don't count either way
            }
            solid_total += 1;
            if air.contains(&super::pack(wx, wy, wz)) || !editor.block_exists_absolute(wx, wy, wz) {
                in_air += 1;
            }
        }
        if solid_total == 0 || in_air * 2 < solid_total {
            return false; // less than half would be visible — skip
        }
    }

    let mut top_row_solid: Vec<(i32, i32, i32, Block)> = Vec::new();
    let mut placed_solid: FnvHashMap<(i32, i32, i32), Block> = FnvHashMap::default();
    for &(vx, vy, vz, pi) in &schem.voxels {
        let e = &schem.palette[pi as usize];
        let (rx, rz) = rotate_xz(vx, vz, schem.w, schem.l, rot);
        let (wx, wy, wz) = (ax + rx - cxo, base_y + vy, az + rz - czo);
        if wx < min_x || wx > max_x || wz < min_z || wz > max_z {
            continue;
        }
        let ptop = surf[(wx - min_x) as usize * hh + (wz - min_z) as usize] - top_gate;
        if wy > ptop || wy <= floor {
            continue; // never breach the roof seal
        }
        if e.is_air {
            // ONLY true Embed pockets dig their air voxels (the powder-snow pit's hollow).
            // EmbedFloor basins must NOT: their air padding is just schematic bounding-box filler,
            // and digging it craters plus-shaped holes into the floor around every basin.
            if orient == Orient::Embed {
                editor.set_block_absolute(AIR, wx, wy, wz, Some(ROCK), None);
            }
            continue;
        }
        // sunk rows (below the floor anchor) carve into rock; everything else is AIR-only.
        let wl: &[Block] = if into_rock || wy < ay {
            rock_and_air
        } else {
            &[AIR]
        };
        editor.set_block_with_properties_absolute(
            BlockWithProperties {
                block: e.block,
                properties: rot_props(&e.props, rot),
            },
            wx,
            wy,
            wz,
            Some(wl),
            None,
        );
        if e.is_fluid {
            editor.schedule_fluid_tick(e.block, wx, wy, wz);
            basin_fluid.insert(super::pack(wx, wy, wz));
        } else if editor.check_for_block_absolute(wx, wy, wz, Some(&[e.block]), None) {
            // only count voxels whose write actually LANDED — the AIR-only whitelist masks writes
            // out against rock, and treating those as "placed" lets the support prune below revert
            // REAL ROCK to air, punching air pockets into floors under formations.
            placed_solid.insert((wx, wy, wz), e.block);
        }
        if into_rock && vy == schem.h - 1 && !e.is_fluid {
            top_row_solid.push((wx, wy, wz, e.block));
        }
    }
    // SUPPORT PRUNE: wall/roof clipping can strand outlying voxels of a formation (a bud island of
    // a big cluster whose connecting arm got clipped) with zero 6-connected support — revert those
    // to AIR so nothing floats. Same technique as the geode shell sweep.
    for (&(wx, wy, wz), &b) in &placed_solid {
        let supported = [
            (wx + 1, wy, wz),
            (wx - 1, wy, wz),
            (wx, wy + 1, wz),
            (wx, wy - 1, wz),
            (wx, wy, wz + 1),
            (wx, wy, wz - 1),
        ]
        .iter()
        .any(|&(nx, ny, nz)| {
            placed_solid.contains_key(&(nx, ny, nz)) || editor.block_exists_absolute(nx, ny, nz)
        });
        if !supported {
            // revert ONLY the exact block we placed — never anything else.
            editor.set_block_absolute(AIR, wx, wy, wz, Some(&[b]), None);
        }
    }
    // RIM SKIRT for floor-inset basins (clay pools etc): the rim sits at the anchor's floor level,
    // but the surrounding floor can undulate 1-2 blocks lower — extend each rim block downward
    // through any air gap so the basin "completes into" its ground instead of hovering at the lip.
    if matches!(orient, Orient::EmbedFloor) {
        for (wx, wy, wz, b) in top_row_solid {
            for dy in 1..=3 {
                if editor.block_exists_absolute(wx, wy - dy, wz) {
                    break;
                }
                editor.set_block_absolute(b, wx, wy - dy, wz, Some(&[AIR]), None);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A gzipped Sponge v2 schematic with the given dimensions, palette and raw BlockData bytes.
    fn schem(dims: (i16, i16, i16), palette: &[(&str, i32)], data: &[u8]) -> Vec<u8> {
        let mut root = HashMap::new();
        root.insert("Version".to_string(), Value::Int(2));
        root.insert("Width".to_string(), Value::Short(dims.0));
        root.insert("Height".to_string(), Value::Short(dims.1));
        root.insert("Length".to_string(), Value::Short(dims.2));
        let pal = palette
            .iter()
            .map(|&(name, i)| (name.to_string(), Value::Int(i)))
            .collect();
        root.insert("Palette".to_string(), Value::Compound(pal));
        root.insert(
            "BlockData".to_string(),
            Value::ByteArray(fastnbt::ByteArray::new(
                data.iter().map(|&b| b as i8).collect(),
            )),
        );
        let raw = fastnbt::to_bytes(&Value::Compound(root)).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        gz.finish().unwrap()
    }

    const ICE_COLUMN: &[(&str, i32)] = &[("minecraft:ice", 0), ("minecraft:air", 1)];

    #[test]
    fn a_valid_formation_loads() {
        let s = load_cave_schem(&schem((1, 3, 1), ICE_COLUMN, &[0, 0, 1])).unwrap();
        assert_eq!((s.w, s.h, s.l, s.solid_count), (1, 2, 1, 2));
    }

    /// Pack files are user-supplied, so a corrupt one must be refused, never panic or allocate
    /// for an index it names.
    #[test]
    fn corrupt_formations_are_refused() {
        let cases: [(&str, Vec<u8>); 6] = [
            (
                "negative palette index",
                schem((1, 1, 1), &[("minecraft:ice", -1)], &[0]),
            ),
            (
                "palette index past the palette",
                schem((1, 1, 1), &[("minecraft:ice", i32::MAX)], &[0]),
            ),
            ("zero width", schem((0, 1, 1), ICE_COLUMN, &[])),
            ("varint too long", schem((1, 1, 1), ICE_COLUMN, &[0xFF; 6])),
            ("varint cut short", schem((1, 1, 1), ICE_COLUMN, &[0x80])),
            ("data too short", schem((2, 2, 2), ICE_COLUMN, &[0, 0])),
        ];
        for (what, bytes) in cases {
            assert!(load_cave_schem(&bytes).is_err(), "{what} was accepted");
        }
    }

    /// Water beside lava inside one formation would break the zero-contact rule wherever it is
    /// stamped; a gap between them is fine.
    #[test]
    fn water_touching_lava_is_refused() {
        let fluids = &[
            ("minecraft:water", 0),
            ("minecraft:lava", 1),
            ("minecraft:ice", 2),
        ];
        assert!(load_cave_schem(&schem((2, 1, 1), fluids, &[0, 1])).is_err());
        assert!(load_cave_schem(&schem((3, 1, 1), fluids, &[0, 2, 1])).is_ok());
    }

    /// Sponge dimensions are unsigned: a 40000-wide formation is not negative.
    #[test]
    fn dimensions_read_as_unsigned() {
        let mut c = HashMap::new();
        c.insert("Width".to_string(), Value::Short(40000u16 as i16));
        assert_eq!(dimension(&c, "Width"), Ok(40000));
    }
}
