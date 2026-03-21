use crate::block_definitions::{Block, AIR};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::luanti_block_map::{to_luanti_node, LuantiGame};
use crate::progress::emit_gui_progress_update;
use crate::world_editor::common::WorldToModify;
use colored::Colorize;
use fastnbt::Value;
use rayon::prelude::*;
use rusqlite::Connection;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Mapblock format version — 29 uses zstd compression for the entire block
const MAP_FORMAT_VERSION: u8 = 29;

/// IDs of stair blocks that need directional param2 mapping.
const STAIR_BLOCK_IDS: &[u8] = &[
    144, // OAK_STAIRS
    177, // STONE_BRICK_STAIRS
    178, // MUD_BRICK_STAIRS
    179, // POLISHED_BLACKSTONE_BRICK_STAIRS
    180, // BRICK_STAIRS
    181, // POLISHED_GRANITE_STAIRS
    182, // END_STONE_BRICK_STAIRS
    183, // POLISHED_DIORITE_STAIRS
    184, // SMOOTH_SANDSTONE_STAIRS
    185, // QUARTZ_STAIRS
    186, // POLISHED_ANDESITE_STAIRS
    187, // NETHER_BRICK_STAIRS
];

/// Convert Minecraft block properties to Luanti param2 value.
///
/// For blocks with a "facing" property, converts to Luanti's facedir,
/// accounting for the Z-axis flip (MC Z+ = South, Luanti Z+ = North).
///
/// Facedir mapping (with Z-flip applied):
/// - MC "north" (-Z_mc = +Z_luanti) → param2 = 0 (ascend toward +Z)
/// - MC "east"  (+X)                → param2 = 1 (ascend toward +X)
/// - MC "south" (+Z_mc = -Z_luanti) → param2 = 2 (ascend toward -Z)
/// - MC "west"  (-X)                → param2 = 3 (ascend toward -X)
///
/// Upside-down stairs (half=top) add 20 to the base facedir.
fn properties_to_param2(block: Block, props: &Value) -> u8 {
    let compound = match props {
        Value::Compound(map) => map,
        _ => return 0,
    };

    let facing = match compound.get("facing") {
        Some(Value::String(s)) => s.as_str(),
        _ => return 0,
    };

    let base = match facing {
        "north" => 0,
        "east" => 1,
        "south" => 2,
        "west" => 3,
        _ => 0,
    };

    // Check for upside-down stairs (half=top adds 20 to facedir)
    let upside_down = STAIR_BLOCK_IDS.contains(&block.id())
        && matches!(
            compound.get("half"),
            Some(Value::String(s)) if s == "top"
        );

    if upside_down { base + 20 } else { base }
}

/// 16×16×16 nodes per mapblock
const NODES_PER_BLOCK: usize = 16 * 16 * 16;

/// Encode a mapblock position (x, y, z) into the SQLite integer key.
///
/// Uses the legacy single-integer encoding for compatibility with Luanti 5.5.0+:
/// `pos = z * 0x1000000 + y * 0x1000 + x`
fn encode_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (z as i64) * 0x1000000 + (y as i64) * 0x1000 + (x as i64)
}

/// Serialize a mapblock into the v29 binary format (zstd-compressed).
///
/// The mapblock contains 16×16×16 nodes. Each node has:
/// - param0: content ID (u16) — mapped to block-local IDs via name-id mapping
/// - param1: lighting (u8) — set to 0, engine fix_light recalculates on first load
/// - param2: rotation/facedir (u8)
///
/// Returns `(encoded_pos, blob)` ready for SQLite insertion.
fn serialize_mapblock(
    block_x: i32,
    block_y: i32,
    block_z: i32,
    section: &crate::world_editor::common::SectionToModify,
    game: LuantiGame,
) -> (i64, Vec<u8>) {
    // Build name-ID mapping: collect unique node names in this mapblock
    let mut name_to_local_id: std::collections::HashMap<&'static str, u16> =
        std::collections::HashMap::new();
    let mut local_id_to_name: Vec<&str> = Vec::new();

    // Pre-populate with air (always ID 0 for efficiency)
    let air_node = to_luanti_node(AIR, game);
    name_to_local_id.insert(air_node.name, 0);
    local_id_to_name.push(air_node.name);

    // Arrays for the 4096 nodes
    let mut param0 = vec![0u16; NODES_PER_BLOCK];
    let param1 = vec![0u8; NODES_PER_BLOCK]; // Start dark; engine fix_light will set correct values
    let mut param2 = vec![0u8; NODES_PER_BLOCK];

    // SectionToModify uses (u8,u8,u8) get_block returning Option<Block>
    // Luanti serialization order is ZYX (z varies slowest)
    // Luanti Z+ = North, but internal data has Z+ = South (Minecraft convention).
    // We flip Z within each mapblock: serialized z reads from internal (15 - z).
    for sz in 0u8..16 {
        for y in 0u8..16 {
            for x in 0u8..16 {
                let serial_idx = (sz as usize) * 256 + (y as usize) * 16 + (x as usize); // ZYX
                let internal_z = 15 - sz;

                let block = section.get_block(x, y, internal_z).unwrap_or(AIR);
                let node = to_luanti_node(block, game);

                let local_id = if let Some(&id) = name_to_local_id.get(node.name) {
                    id
                } else {
                    let id = local_id_to_name.len() as u16;
                    name_to_local_id.insert(node.name, id);
                    local_id_to_name.push(node.name);
                    id
                };

                param0[serial_idx] = local_id;

                // Convert Minecraft block properties to Luanti param2
                // Properties index uses internal coordinates (YZX order)
                let props_idx = crate::world_editor::common::SectionToModify::index(x, y, internal_z);
                param2[serial_idx] = if let Some(props) = section.properties.get(&props_idx) {
                    properties_to_param2(block, props)
                } else {
                    node.param2
                };

            }
        }
    }

    // Build the uncompressed mapblock buffer (everything after the version byte)
    let mut buf: Vec<u8> = Vec::with_capacity(16384);

    // flags: day_night_differs=1, generated=1
    let flags: u8 = 0x02 | 0x08;
    buf.push(flags);

    // lighting_complete: set to 0 so the engine recomputes lighting at all
    // block boundaries, giving proper sunlight propagation and shadows.
    buf.extend_from_slice(&0x0000u16.to_be_bytes());

    // timestamp
    buf.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());

    // name-id mapping (v29: comes before node data)
    buf.push(0); // name_id_mapping_version
    buf.extend_from_slice(&(local_id_to_name.len() as u16).to_be_bytes());
    for (i, name) in local_id_to_name.iter().enumerate() {
        buf.extend_from_slice(&(i as u16).to_be_bytes());
        buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
        buf.extend_from_slice(name.as_bytes());
    }

    // content_width and params_width
    buf.push(2); // content_width (u16 per node)
    buf.push(2); // params_width (param1 + param2)

    // Node data: param0 (u16 BE), param1 (u8), param2 (u8) — all in ZYX order
    for &p0 in &param0 {
        buf.extend_from_slice(&p0.to_be_bytes());
    }
    buf.extend_from_slice(&param1);
    buf.extend_from_slice(&param2);

    // Node metadata: empty (version 2, count 0)
    buf.push(2); // metadata version
    buf.extend_from_slice(&0u16.to_be_bytes()); // count = 0

    // Static objects
    buf.push(0); // version
    buf.extend_from_slice(&0u16.to_be_bytes()); // count = 0

    // Node timers (v25+ format, placed after static objects in v29)
    buf.push(10); // data length per timer (2 + 4 + 4)
    buf.extend_from_slice(&0u16.to_be_bytes()); // count = 0

    // Compress the entire buffer with zstd
    let compressed =
        zstd::bulk::compress(&buf, 3).expect("zstd compression failed for mapblock data");

    // Final blob: version byte + compressed data
    let mut blob = Vec::with_capacity(1 + compressed.len());
    blob.push(MAP_FORMAT_VERSION);
    blob.extend_from_slice(&compressed);

    let pos = encode_block_pos(block_x, block_y, block_z);
    (pos, blob)
}

/// Find a spawn position that is outdoors on solid ground.
/// Spirals outward from (center_x, center_z) looking for a column where
/// there is solid ground (not trees/leaves) with air above.
fn find_safe_spawn(
    world: &WorldToModify,
    center_x: i32,
    center_z: i32,
    ground_level: i32,
) -> (i32, i32, i32) {
    use crate::block_definitions::*;
    // Blocks to avoid spawning on (trees, leaves, vegetation)
    let non_ground = [
        OAK_LOG, OAK_LEAVES, BIRCH_LOG, BIRCH_LEAVES,
        DARK_OAK_LOG, DARK_OAK_LEAVES, JUNGLE_LOG, JUNGLE_LEAVES,
        ACACIA_LOG, ACACIA_LEAVES, SPRUCE_LOG, SPRUCE_LEAVES,
    ];

    // Scan range: from ground_level up to ground_level + 400 to cover elevation
    let y_max = ground_level + 400;
    for radius in 0i32..=40 {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() != radius && dz.abs() != radius {
                    continue; // only check perimeter
                }
                let x = center_x + dx;
                let z = center_z + dz;
                // Scan from top down to find the highest solid block
                for y in (ground_level..=y_max).rev() {
                    let block = world.get_block(x, y, z);
                    if let Some(b) = block {
                        if b == AIR {
                            continue;
                        }
                        // Skip tree/leaf blocks — keep scanning down
                        if non_ground.contains(&b) {
                            continue;
                        }
                        // Found solid ground — check air above
                        let above1 = world.get_block(x, y + 1, z);
                        let above2 = world.get_block(x, y + 2, z);
                        if (above1.is_none() || above1 == Some(AIR) || non_ground.contains(&above1.unwrap()))
                            && (above2.is_none() || above2 == Some(AIR) || non_ground.contains(&above2.unwrap()))
                        {
                            return (x, y + 1, z);
                        }
                        break; // solid non-tree block but enclosed, try next column
                    }
                }
            }
        }
    }
    // Fallback: center, above ground
    (center_x, ground_level + 3, center_z)
}

/// Creates all Luanti world files and writes the map database.
pub fn save_luanti_world(
    world: &WorldToModify,
    world_dir: &Path,
    xzbbox: &XZBBox,
    _llbbox: &LLBBox,
    game: LuantiGame,
    level_name: Option<&str>,
    spawn_point: Option<(i32, i32)>,
    ground_level: i32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("{} Saving Luanti world...", "[7/7]".bold());
    emit_gui_progress_update(90.0, "Saving Luanti world...");

    // Create world directory if needed
    fs::create_dir_all(world_dir)?;

    // Determine spawn coordinates — find an outdoor position
    let (base_x, base_z) = if let Some((sx, sz)) = spawn_point {
        (sx, sz)
    } else {
        let cx = (xzbbox.min_x() + xzbbox.max_x()) / 2;
        let cz = (xzbbox.min_z() + xzbbox.max_z()) / 2;
        (cx, cz)
    };
    let (spawn_x, spawn_y, spawn_z) =
        find_safe_spawn(world, base_x, base_z, ground_level);

    // Convert spawn Z from internal (Z+ = South) to Luanti (Z+ = North)
    let spawn_z = -spawn_z - 1;

    // Write world.mt
    write_world_mt(world_dir, game, level_name, spawn_x, spawn_y, spawn_z)?;

    // Write map_meta.txt
    write_map_meta(world_dir, game)?;

    // Write env_meta.txt
    write_env_meta(world_dir)?;

    // Write worldmod for singlenode mapgen + spawn
    // Compute world bounds in Luanti coordinates for fix_light
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;
    for (&(region_x, region_z), region) in &world.regions {
        for (&(chunk_x, chunk_z), chunk) in &region.chunks {
            for (&section_y, _) in &chunk.sections {
                let mb_x = region_x * 32 + chunk_x;
                let orig_z = region_z * 32 + chunk_z;
                let mb_z = -orig_z - 1;
                let mb_y = section_y as i32;
                min_x = min_x.min(mb_x * 16);
                max_x = max_x.max(mb_x * 16 + 15);
                min_y = min_y.min(mb_y * 16);
                max_y = max_y.max(mb_y * 16 + 15);
                min_z = min_z.min(mb_z * 16);
                max_z = max_z.max(mb_z * 16 + 15);
            }
        }
    }
    write_worldmod(world_dir, spawn_x, spawn_y, spawn_z, game, min_x, min_y, min_z, max_x, max_y, max_z)?;

    // Write map.sqlite
    write_map_database(world, world_dir, game)?;

    emit_gui_progress_update(99.0, "Luanti world saved.");
    println!(
        "{} Luanti world saved to: {}",
        "Done!".green().bold(),
        world_dir.display()
    );

    Ok(())
}

fn write_world_mt(
    world_dir: &Path,
    game: LuantiGame,
    level_name: Option<&str>,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut f = fs::File::create(world_dir.join("world.mt"))?;
    writeln!(f, "backend = sqlite3")?;
    writeln!(f, "player_backend = sqlite3")?;
    writeln!(f, "auth_backend = sqlite3")?;
    writeln!(f, "mod_storage_backend = sqlite3")?;
    writeln!(f, "gameid = {}", game.game_id())?;
    if let Some(name) = level_name {
        writeln!(f, "world_name = {}", name)?;
    }
    writeln!(f, "creative_mode = true")?;
    writeln!(f, "enable_damage = false")?;
    writeln!(f, "server_announce = false")?;
    writeln!(f, "static_spawnpoint = {}, {}, {}", spawn_x, spawn_y, spawn_z)?;
    Ok(())
}

fn write_map_meta(
    world_dir: &Path,
    game: LuantiGame,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut f = fs::File::create(world_dir.join("map_meta.txt"))?;
    writeln!(f, "mg_name = singlenode")?;
    writeln!(f, "seed = 0")?;
    if game == LuantiGame::Mineclonia {
        // Tell Mineclonia not to activate its custom levelgen system,
        // so it won't generate terrain over our pre-built map.
        writeln!(f, "mcl_singlenode_mapgen = false")?;
    }
    writeln!(f, "[end_of_params]")?;
    Ok(())
}

fn write_env_meta(
    world_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut f = fs::File::create(world_dir.join("env_meta.txt"))?;
    writeln!(f, "game_time = 0")?;
    writeln!(f, "time_of_day = 6000")?;
    writeln!(f, "EnvArgsEnd")?;
    Ok(())
}

fn write_worldmod(
    world_dir: &Path,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
    game: LuantiGame,
    area_min_x: i32,
    area_min_y: i32,
    area_min_z: i32,
    area_max_x: i32,
    area_max_y: i32,
    area_max_z: i32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mod_dir = world_dir.join("worldmods").join("arnis_mapgen");
    fs::create_dir_all(&mod_dir)?;

    // Write mod.conf (optional_depends on mcl_spawn so we load after Mineclonia's spawn)
    let mut mc = fs::File::create(mod_dir.join("mod.conf"))?;
    writeln!(mc, "name = arnis_mapgen")?;
    writeln!(mc, "description = Arnis world configuration (singlenode mapgen + spawn)")?;
    writeln!(mc, "optional_depends = mcl_spawn")?;

    if game == LuantiGame::Mineclonia {
        // Write mcl_levelgen.conf to inhibit Mineclonia's custom mapgen
        let mut lc = fs::File::create(mod_dir.join("mcl_levelgen.conf"))?;
        writeln!(lc, "disable_mcl_levelgen = true")?;
    }

    let mut f = fs::File::create(mod_dir.join("init.lua"))?;
    writeln!(f, "-- Arnis world configuration")?;
    writeln!(
        f,
        "minetest.set_mapgen_setting(\"mg_name\", \"singlenode\", true)"
    )?;
    writeln!(f)?;
    writeln!(f, "local SPAWN = {{x={}, y={}, z={}}}", spawn_x, spawn_y, spawn_z)?;
    writeln!(f)?;
    writeln!(f, "-- Teleport player to our spawn after a short delay")?;
    writeln!(f, "-- (overrides game-specific spawn handlers like Mineclonia's)")?;
    writeln!(f, "minetest.register_on_joinplayer(function(player)")?;
    writeln!(f, "    minetest.after(0.5, function()")?;
    writeln!(f, "        if player:is_player() then")?;
    writeln!(f, "            player:set_pos(SPAWN)")?;
    writeln!(f, "        end")?;
    writeln!(f, "    end)")?;
    writeln!(f, "end)")?;
    writeln!(f)?;
    writeln!(f, "minetest.register_on_respawnplayer(function(player)")?;
    writeln!(f, "    minetest.after(0.5, function()")?;
    writeln!(f, "        if player:is_player() then")?;
    writeln!(f, "            player:set_pos(SPAWN)")?;
    writeln!(f, "        end")?;
    writeln!(f, "    end)")?;
    writeln!(f, "    return true")?;
    writeln!(f, "end)")?;
    writeln!(f)?;
    writeln!(f, "-- Fix lighting on first load")?;
    writeln!(f, "local AREA_MIN = {{x={}, y={}, z={}}}", area_min_x, area_min_y, area_min_z)?;
    writeln!(f, "local AREA_MAX = {{x={}, y={}, z={}}}", area_max_x, area_max_y, area_max_z)?;
    writeln!(f, "local storage = minetest.get_mod_storage()")?;
    writeln!(f, "minetest.register_on_joinplayer(function(player)")?;
    writeln!(f, "    if storage:get_string(\"lighting_fixed\") ~= \"true\" then")?;
    writeln!(f, "        minetest.chat_send_all(\"Loading map and fixing lighting...\")")?;
    writeln!(f, "        minetest.emerge_area(AREA_MIN, AREA_MAX, function(blockpos, action, calls_remaining)")?;
    writeln!(f, "            if calls_remaining == 0 then")?;
    writeln!(f, "                minetest.fix_light(AREA_MIN, AREA_MAX)")?;
    writeln!(f, "                storage:set_string(\"lighting_fixed\", \"true\")")?;
    writeln!(f, "                minetest.chat_send_all(\"Lighting recalculated.\")")?;
    writeln!(f, "            end")?;
    writeln!(f, "        end)")?;
    writeln!(f, "    end")?;
    writeln!(f, "end)")?;

    Ok(())
}

fn write_map_database(
    world: &WorldToModify,
    world_dir: &Path,
    game: LuantiGame,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db_path = world_dir.join("map.sqlite");

    // Remove existing database if present
    if db_path.exists() {
        fs::remove_file(&db_path)?;
    }

    let conn = Connection::open(&db_path)?;

    // Optimize for bulk writes
    conn.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         PRAGMA locking_mode = EXCLUSIVE;",
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS `blocks` (`pos` INT NOT NULL PRIMARY KEY, `data` BLOB)",
        [],
    )?;

    // Collect all mapblock positions and sections for parallel serialization
    let mut block_entries: Vec<(i32, i32, i32, &crate::world_editor::common::SectionToModify)> =
        Vec::new();

    for (&(region_x, region_z), region) in &world.regions {
        for (&(chunk_x, chunk_z), chunk) in &region.chunks {
            for (&section_y, section) in &chunk.sections {
                // region_x * 32 + chunk_x = absolute chunk X = mapblock X
                // section_y (i8) = mapblock Y
                // Negate Z: Luanti Z+ = North, internal Z+ = South
                let mb_x = region_x * 32 + chunk_x;
                let orig_z = region_z * 32 + chunk_z;
                let mb_z = -orig_z - 1;
                let mb_y = section_y as i32;
                block_entries.push((mb_x, mb_y, mb_z, section));
            }
        }
    }

    println!(
        "  Serializing {} mapblocks...",
        block_entries.len().to_string().bold()
    );

    // Serialize mapblocks in parallel
    let serialized: Vec<(i64, Vec<u8>)> = block_entries
        .par_iter()
        .map(|&(bx, by, bz, section)| serialize_mapblock(bx, by, bz, section, game))
        .collect();

    // Insert all serialized blocks into SQLite (must be sequential)
    conn.execute_batch("BEGIN TRANSACTION")?;
    {
        let mut stmt = conn.prepare("INSERT INTO `blocks` (`pos`, `data`) VALUES (?1, ?2)")?;
        for (pos, data) in &serialized {
            stmt.execute(rusqlite::params![pos, data])?;
        }
    }
    conn.execute_batch("COMMIT")?;

    println!(
        "  Wrote {} mapblocks to map.sqlite",
        serialized.len().to_string().bold()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_block_pos_origin() {
        assert_eq!(encode_block_pos(0, 0, 0), 0);
    }

    #[test]
    fn test_encode_block_pos_positive() {
        // pos = z * 0x1000000 + y * 0x1000 + x
        assert_eq!(encode_block_pos(1, 2, 3), 3 * 0x1000000 + 2 * 0x1000 + 1);
    }

    #[test]
    fn test_encode_block_pos_negative() {
        // Negative coordinates should work with signed arithmetic
        let pos = encode_block_pos(-1, -1, -1);
        assert_eq!(pos, (-1i64) * 0x1000000 + (-1i64) * 0x1000 + (-1i64));
    }
}
