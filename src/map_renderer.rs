// Top-down world map renderer for GUI preview.
//
// Generates a 1:1 pixel-per-block PNG image of the generated world,
// showing the topmost visible block at each position.

use fastanvil::Region;
use fastnbt::{from_bytes, Value};
use image::{Rgb, RgbImage};
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

/// Renders a top-down view of the generated Minecraft world.
/// Returns the path to the saved image file.
pub fn render_world_map(
    world_dir: &Path,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Result<std::path::PathBuf, String> {
    let width = (max_x - min_x + 1) as u32;
    let height = (max_z - min_z + 1) as u32;

    if width == 0 || height == 0 {
        return Err("Invalid world bounds".to_string());
    }

    let mut img = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    let colors = get_block_colors();

    // Calculate region range
    let min_region_x = min_x >> 9; // divide by 512 (32 chunks * 16 blocks)
    let max_region_x = max_x >> 9;
    let min_region_z = min_z >> 9;
    let max_region_z = max_z >> 9;

    let region_dir = world_dir.join("region");

    for region_x in min_region_x..=max_region_x {
        for region_z in min_region_z..=max_region_z {
            let region_path = region_dir.join(format!("r.{}.{}.mca", region_x, region_z));

            if !region_path.exists() {
                continue;
            }

            if let Ok(file) = File::open(&region_path) {
                if let Ok(mut region) = Region::from_stream(file) {
                    render_region(
                        &mut region,
                        &mut img,
                        region_x,
                        region_z,
                        min_x,
                        min_z,
                        max_x,
                        max_z,
                        &colors,
                    );
                }
            }
        }
    }

    // Save the image
    let output_path = world_dir.join("arnis_world_map.png");
    img.save(&output_path)
        .map_err(|e| format!("Failed to save map image: {}", e))?;

    Ok(output_path)
}

/// Renders all chunks within a region to the image
fn render_region(
    region: &mut Region<File>,
    img: &mut RgbImage,
    region_x: i32,
    region_z: i32,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    colors: &HashMap<&'static str, Rgb<u8>>,
) {
    let region_base_x = region_x * 512;
    let region_base_z = region_z * 512;

    for chunk_local_x in 0..32 {
        for chunk_local_z in 0..32 {
            let chunk_base_x = region_base_x + chunk_local_x * 16;
            let chunk_base_z = region_base_z + chunk_local_z * 16;

            // Skip chunks outside our bounds
            if chunk_base_x + 15 < min_x
                || chunk_base_x > max_x
                || chunk_base_z + 15 < min_z
                || chunk_base_z > max_z
            {
                continue;
            }

            if let Ok(Some(chunk_data)) =
                region.read_chunk(chunk_local_x as usize, chunk_local_z as usize)
            {
                render_chunk(
                    &chunk_data,
                    img,
                    chunk_base_x,
                    chunk_base_z,
                    min_x,
                    min_z,
                    max_x,
                    max_z,
                    colors,
                );
            }
        }
    }
}

/// Renders a single chunk to the image
#[allow(clippy::too_many_arguments)]
fn render_chunk(
    chunk_data: &[u8],
    img: &mut RgbImage,
    chunk_base_x: i32,
    chunk_base_z: i32,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    colors: &HashMap<&'static str, Rgb<u8>>,
) {
    // Parse chunk NBT - look for Level.sections or sections depending on format
    let chunk: Value = match from_bytes(chunk_data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Try to get sections from the chunk data
    let sections = get_sections_from_chunk(&chunk);
    if sections.is_empty() {
        return;
    }

    // For each column in the chunk
    for local_x in 0..16 {
        for local_z in 0..16 {
            let world_x = chunk_base_x + local_x;
            let world_z = chunk_base_z + local_z;

            // Skip if outside our bounds
            if world_x < min_x || world_x > max_x || world_z < min_z || world_z > max_z {
                continue;
            }

            // Find topmost non-air block
            if let Some(block_name) = find_top_block(&sections, local_x as usize, local_z as usize)
            {
                // Strip minecraft: prefix for lookup
                let short_name = block_name.strip_prefix("minecraft:").unwrap_or(&block_name);

                let color = colors
                    .get(short_name)
                    .copied()
                    .unwrap_or_else(|| get_fallback_color(&block_name));

                let img_x = (world_x - min_x) as u32;
                let img_z = (world_z - min_z) as u32;

                if img_x < img.width() && img_z < img.height() {
                    img.put_pixel(img_x, img_z, color);
                }
            }
        }
    }
}

/// Extracts sections from chunk data (handles both old and new formats)
fn get_sections_from_chunk(chunk: &Value) -> Vec<&Value> {
    let mut sections = Vec::new();

    // Try new format (1.18+): directly in chunk
    if let Value::Compound(map) = chunk {
        if let Some(Value::List(secs)) = map.get("sections") {
            for sec in secs {
                sections.push(sec);
            }
            return sections;
        }

        // Try via Level wrapper (older format)
        if let Some(Value::Compound(level)) = map.get("Level") {
            if let Some(Value::List(secs)) = level.get("sections") {
                for sec in secs {
                    sections.push(sec);
                }
            }
        }
    }

    sections
}

/// Finds the topmost non-air block at the given local x,z coordinates
fn find_top_block(sections: &[&Value], local_x: usize, local_z: usize) -> Option<String> {
    // Sort sections by Y (descending) to find topmost first
    let mut sorted_sections: Vec<(i8, &Value)> = sections
        .iter()
        .filter_map(|s| {
            if let Value::Compound(map) = s {
                if let Some(Value::Byte(y)) = map.get("Y") {
                    return Some((*y, *s));
                }
            }
            None
        })
        .collect();

    sorted_sections.sort_by(|a, b| b.0.cmp(&a.0));

    for (_y, section) in sorted_sections {
        if let Some(block_name) = get_block_at_section(section, local_x, local_z) {
            if !is_transparent_block(&block_name) {
                return Some(block_name);
            }
        }
    }

    None
}

/// Gets the topmost non-air block in a section at the given x,z
fn get_block_at_section(section: &Value, local_x: usize, local_z: usize) -> Option<String> {
    let section_map = match section {
        Value::Compound(m) => m,
        _ => return None,
    };

    let block_states = match section_map.get("block_states") {
        Some(Value::Compound(bs)) => bs,
        _ => return None,
    };

    let palette = match block_states.get("palette") {
        Some(Value::List(p)) => p,
        _ => return None,
    };

    // If palette has only one block, that's the block for the entire section
    if palette.len() == 1 {
        return get_block_name_from_palette(&palette[0]);
    }

    let data = match block_states.get("data") {
        Some(Value::LongArray(d)) => d,
        _ => return None,
    };

    // Calculate bits per block
    let bits_per_block = std::cmp::max(4, (palette.len() as f64).log2().ceil() as usize);
    let blocks_per_long = 64 / bits_per_block;
    let mask = (1u64 << bits_per_block) - 1;

    // Search from top (y=15) to bottom (y=0) within this section
    for local_y in (0..16).rev() {
        let block_index = local_y * 256 + local_z * 16 + local_x;
        let long_index = block_index / blocks_per_long;
        let bit_offset = (block_index % blocks_per_long) * bits_per_block;

        if long_index >= data.len() {
            continue;
        }

        let palette_index = ((data[long_index] as u64 >> bit_offset) & mask) as usize;

        if palette_index < palette.len() {
            if let Some(name) = get_block_name_from_palette(&palette[palette_index]) {
                if !is_transparent_block(&name) {
                    return Some(name);
                }
            }
        }
    }

    None
}

/// Extracts block name from a palette entry
fn get_block_name_from_palette(entry: &Value) -> Option<String> {
    if let Value::Compound(map) = entry {
        if let Some(Value::String(name)) = map.get("Name") {
            return Some(name.clone());
        }
    }
    None
}

/// Checks if a block should be considered transparent (look through it)
fn is_transparent_block(name: &str) -> bool {
    let short_name = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(
        short_name,
        "air"
            | "cave_air"
            | "void_air"
            | "glass"
            | "glass_pane"
            | "white_stained_glass"
            | "gray_stained_glass"
            | "light_gray_stained_glass"
            | "brown_stained_glass"
            | "tinted_glass"
            | "barrier"
            | "light"
            | "short_grass"
            | "tall_grass"
            | "dead_bush"
            | "poppy"
            | "dandelion"
            | "blue_orchid"
            | "azure_bluet"
            | "iron_bars"
            | "ladder"
            | "scaffolding"
            | "rail"
            | "powered_rail"
            | "detector_rail"
            | "activator_rail"
    )
}

/// Returns a fallback color based on block name patterns
fn get_fallback_color(name: &str) -> Rgb<u8> {
    // Try to guess color from name
    if name.contains("stone") || name.contains("cobble") || name.contains("andesite") {
        return Rgb([128, 128, 128]);
    }
    if name.contains("dirt") || name.contains("mud") {
        return Rgb([139, 90, 43]);
    }
    if name.contains("sand") {
        return Rgb([219, 211, 160]);
    }
    if name.contains("grass") {
        return Rgb([86, 125, 70]);
    }
    if name.contains("water") {
        return Rgb([59, 86, 165]);
    }
    if name.contains("log") || name.contains("wood") {
        return Rgb([101, 76, 48]);
    }
    if name.contains("leaves") {
        return Rgb([55, 95, 36]);
    }
    if name.contains("planks") {
        return Rgb([162, 130, 78]);
    }
    if name.contains("brick") {
        return Rgb([150, 97, 83]);
    }
    if name.contains("concrete") {
        return Rgb([128, 128, 128]);
    }
    if name.contains("wool") || name.contains("carpet") {
        return Rgb([220, 220, 220]);
    }
    if name.contains("terracotta") {
        return Rgb([152, 94, 67]);
    }
    if name.contains("iron") {
        return Rgb([200, 200, 200]);
    }
    if name.contains("gold") {
        return Rgb([255, 215, 0]);
    }
    if name.contains("diamond") {
        return Rgb([97, 219, 213]);
    }
    if name.contains("emerald") {
        return Rgb([17, 160, 54]);
    }
    if name.contains("lapis") {
        return Rgb([38, 67, 156]);
    }
    if name.contains("redstone") {
        return Rgb([170, 0, 0]);
    }
    if name.contains("netherrack") || name.contains("nether") {
        return Rgb([111, 54, 53]);
    }
    if name.contains("end_stone") {
        return Rgb([219, 222, 158]);
    }
    if name.contains("obsidian") {
        return Rgb([15, 10, 24]);
    }
    if name.contains("deepslate") {
        return Rgb([72, 72, 73]);
    }
    if name.contains("blackstone") {
        return Rgb([42, 36, 41]);
    }
    if name.contains("quartz") {
        return Rgb([235, 229, 222]);
    }
    if name.contains("prismarine") {
        return Rgb([76, 128, 113]);
    }
    if name.contains("copper") {
        return Rgb([192, 107, 79]);
    }
    if name.contains("amethyst") {
        return Rgb([133, 97, 191]);
    }
    if name.contains("moss") {
        return Rgb([89, 109, 45]);
    }
    if name.contains("dripstone") {
        return Rgb([134, 107, 92]);
    }

    // Default gray for unknown blocks
    Rgb([160, 160, 160])
}

/// Returns a mapping of common block names to RGB colors (without minecraft: prefix)
fn get_block_colors() -> HashMap<&'static str, Rgb<u8>> {
    HashMap::from([
        // Natural blocks
        ("grass_block", Rgb([86, 125, 70])),
        ("short_grass", Rgb([86, 125, 70])),
        ("tall_grass", Rgb([86, 125, 70])),
        ("dirt", Rgb([139, 90, 43])),
        ("coarse_dirt", Rgb([119, 85, 59])),
        ("podzol", Rgb([91, 63, 24])),
        ("rooted_dirt", Rgb([144, 103, 76])),
        ("mud", Rgb([60, 57, 61])),
        ("stone", Rgb([128, 128, 128])),
        ("granite", Rgb([149, 108, 91])),
        ("polished_granite", Rgb([154, 112, 98])),
        ("diorite", Rgb([189, 188, 189])),
        ("polished_diorite", Rgb([195, 195, 195])),
        ("andesite", Rgb([136, 136, 137])),
        ("polished_andesite", Rgb([132, 135, 134])),
        ("deepslate", Rgb([72, 72, 73])),
        ("cobbled_deepslate", Rgb([77, 77, 80])),
        ("polished_deepslate", Rgb([72, 72, 73])),
        ("deepslate_bricks", Rgb([70, 70, 71])),
        ("deepslate_tiles", Rgb([54, 54, 55])),
        ("calcite", Rgb([223, 224, 220])),
        ("tuff", Rgb([108, 109, 102])),
        ("dripstone_block", Rgb([134, 107, 92])),
        ("sand", Rgb([219, 211, 160])),
        ("red_sand", Rgb([190, 102, 33])),
        ("gravel", Rgb([131, 127, 126])),
        ("clay", Rgb([160, 166, 179])),
        ("bedrock", Rgb([85, 85, 85])),
        // Water and ice
        ("water", Rgb([59, 86, 165])),
        ("ice", Rgb([145, 183, 253])),
        ("packed_ice", Rgb([141, 180, 250])),
        ("blue_ice", Rgb([116, 167, 253])),
        ("snow", Rgb([249, 254, 254])),
        ("snow_block", Rgb([249, 254, 254])),
        ("powder_snow", Rgb([248, 253, 253])),
        // Wood types
        ("oak_log", Rgb([109, 85, 50])),
        ("oak_planks", Rgb([162, 130, 78])),
        ("oak_slab", Rgb([162, 130, 78])),
        ("oak_stairs", Rgb([162, 130, 78])),
        ("oak_fence", Rgb([162, 130, 78])),
        ("oak_door", Rgb([162, 130, 78])),
        ("spruce_log", Rgb([58, 37, 16])),
        ("spruce_planks", Rgb([115, 85, 49])),
        ("spruce_slab", Rgb([115, 85, 49])),
        ("spruce_stairs", Rgb([115, 85, 49])),
        ("spruce_fence", Rgb([115, 85, 49])),
        ("spruce_door", Rgb([115, 85, 49])),
        ("birch_log", Rgb([216, 215, 210])),
        ("birch_planks", Rgb([196, 179, 123])),
        ("birch_slab", Rgb([196, 179, 123])),
        ("birch_stairs", Rgb([196, 179, 123])),
        ("birch_fence", Rgb([196, 179, 123])),
        ("birch_door", Rgb([196, 179, 123])),
        ("jungle_log", Rgb([85, 68, 25])),
        ("jungle_planks", Rgb([160, 115, 81])),
        ("acacia_log", Rgb([103, 96, 86])),
        ("acacia_planks", Rgb([168, 90, 50])),
        ("dark_oak_log", Rgb([60, 46, 26])),
        ("dark_oak_planks", Rgb([67, 43, 20])),
        ("dark_oak_slab", Rgb([67, 43, 20])),
        ("dark_oak_stairs", Rgb([67, 43, 20])),
        ("dark_oak_fence", Rgb([67, 43, 20])),
        ("dark_oak_door", Rgb([67, 43, 20])),
        ("mangrove_log", Rgb([84, 66, 36])),
        ("mangrove_planks", Rgb([117, 54, 48])),
        ("cherry_log", Rgb([54, 33, 44])),
        ("cherry_planks", Rgb([226, 178, 172])),
        ("bamboo_block", Rgb([122, 129, 52])),
        ("bamboo_planks", Rgb([194, 175, 93])),
        ("crimson_stem", Rgb([92, 25, 29])),
        ("crimson_planks", Rgb([101, 48, 70])),
        ("warped_stem", Rgb([58, 58, 77])),
        ("warped_planks", Rgb([43, 104, 99])),
        // Leaves
        ("oak_leaves", Rgb([55, 95, 36])),
        ("spruce_leaves", Rgb([61, 99, 61])),
        ("birch_leaves", Rgb([80, 106, 47])),
        ("jungle_leaves", Rgb([48, 113, 20])),
        ("acacia_leaves", Rgb([75, 104, 40])),
        ("dark_oak_leaves", Rgb([35, 82, 11])),
        ("mangrove_leaves", Rgb([69, 123, 38])),
        ("cherry_leaves", Rgb([228, 177, 197])),
        ("azalea_leaves", Rgb([71, 96, 37])),
        // Stone bricks and variants
        ("stone_bricks", Rgb([122, 122, 122])),
        ("stone_brick_slab", Rgb([122, 122, 122])),
        ("stone_brick_stairs", Rgb([122, 122, 122])),
        ("stone_brick_wall", Rgb([122, 122, 122])),
        ("mossy_stone_bricks", Rgb([115, 121, 105])),
        ("mossy_stone_brick_slab", Rgb([115, 121, 105])),
        ("mossy_stone_brick_stairs", Rgb([115, 121, 105])),
        ("mossy_stone_brick_wall", Rgb([115, 121, 105])),
        ("cracked_stone_bricks", Rgb([118, 117, 118])),
        ("chiseled_stone_bricks", Rgb([119, 119, 119])),
        ("cobblestone", Rgb([128, 127, 127])),
        ("cobblestone_slab", Rgb([128, 127, 127])),
        ("cobblestone_stairs", Rgb([128, 127, 127])),
        ("cobblestone_wall", Rgb([128, 127, 127])),
        ("mossy_cobblestone", Rgb([110, 118, 94])),
        ("mossy_cobblestone_slab", Rgb([110, 118, 94])),
        ("mossy_cobblestone_stairs", Rgb([110, 118, 94])),
        ("mossy_cobblestone_wall", Rgb([110, 118, 94])),
        ("stone_slab", Rgb([128, 128, 128])),
        ("stone_stairs", Rgb([128, 128, 128])),
        ("smooth_stone", Rgb([158, 158, 158])),
        ("smooth_stone_slab", Rgb([158, 158, 158])),
        // Bricks and terracotta
        ("bricks", Rgb([150, 97, 83])),
        ("brick_slab", Rgb([150, 97, 83])),
        ("brick_stairs", Rgb([150, 97, 83])),
        ("brick_wall", Rgb([150, 97, 83])),
        ("mud_bricks", Rgb([137, 103, 79])),
        ("mud_brick_slab", Rgb([137, 103, 79])),
        ("mud_brick_stairs", Rgb([137, 103, 79])),
        ("mud_brick_wall", Rgb([137, 103, 79])),
        ("terracotta", Rgb([152, 94, 67])),
        ("white_terracotta", Rgb([210, 178, 161])),
        ("orange_terracotta", Rgb([162, 84, 38])),
        ("magenta_terracotta", Rgb([149, 88, 109])),
        ("light_blue_terracotta", Rgb([113, 109, 138])),
        ("yellow_terracotta", Rgb([186, 133, 35])),
        ("lime_terracotta", Rgb([104, 118, 53])),
        ("pink_terracotta", Rgb([162, 78, 79])),
        ("gray_terracotta", Rgb([58, 42, 36])),
        ("light_gray_terracotta", Rgb([135, 107, 98])),
        ("cyan_terracotta", Rgb([87, 91, 91])),
        ("purple_terracotta", Rgb([118, 70, 86])),
        ("blue_terracotta", Rgb([74, 60, 91])),
        ("brown_terracotta", Rgb([77, 51, 36])),
        ("green_terracotta", Rgb([76, 83, 42])),
        ("red_terracotta", Rgb([143, 61, 47])),
        ("black_terracotta", Rgb([37, 23, 16])),
        // Concrete
        ("white_concrete", Rgb([207, 213, 214])),
        ("orange_concrete", Rgb([224, 97, 0])),
        ("magenta_concrete", Rgb([169, 48, 159])),
        ("light_blue_concrete", Rgb([35, 137, 198])),
        ("yellow_concrete", Rgb([241, 175, 21])),
        ("lime_concrete", Rgb([94, 169, 24])),
        ("pink_concrete", Rgb([214, 101, 143])),
        ("gray_concrete", Rgb([55, 58, 62])),
        ("light_gray_concrete", Rgb([125, 125, 115])),
        ("cyan_concrete", Rgb([21, 119, 136])),
        ("purple_concrete", Rgb([100, 32, 156])),
        ("blue_concrete", Rgb([45, 47, 143])),
        ("brown_concrete", Rgb([96, 60, 32])),
        ("green_concrete", Rgb([73, 91, 36])),
        ("red_concrete", Rgb([142, 33, 33])),
        ("black_concrete", Rgb([8, 10, 15])),
        // Wool
        ("white_wool", Rgb([234, 236, 237])),
        ("orange_wool", Rgb([241, 118, 20])),
        ("magenta_wool", Rgb([190, 68, 179])),
        ("light_blue_wool", Rgb([58, 175, 217])),
        ("yellow_wool", Rgb([249, 198, 40])),
        ("lime_wool", Rgb([112, 185, 26])),
        ("pink_wool", Rgb([238, 141, 172])),
        ("gray_wool", Rgb([63, 68, 72])),
        ("light_gray_wool", Rgb([142, 142, 135])),
        ("cyan_wool", Rgb([21, 138, 145])),
        ("purple_wool", Rgb([122, 42, 173])),
        ("blue_wool", Rgb([53, 57, 157])),
        ("brown_wool", Rgb([114, 72, 41])),
        ("green_wool", Rgb([85, 110, 28])),
        ("red_wool", Rgb([161, 39, 35])),
        ("black_wool", Rgb([21, 21, 26])),
        // Sandstone
        ("sandstone", Rgb([223, 214, 170])),
        ("sandstone_slab", Rgb([223, 214, 170])),
        ("sandstone_stairs", Rgb([223, 214, 170])),
        ("sandstone_wall", Rgb([223, 214, 170])),
        ("chiseled_sandstone", Rgb([223, 214, 170])),
        ("cut_sandstone", Rgb([225, 217, 171])),
        ("cut_sandstone_slab", Rgb([225, 217, 171])),
        ("smooth_sandstone", Rgb([223, 214, 170])),
        ("smooth_sandstone_slab", Rgb([223, 214, 170])),
        ("smooth_sandstone_stairs", Rgb([223, 214, 170])),
        ("red_sandstone", Rgb([186, 99, 29])),
        ("red_sandstone_slab", Rgb([186, 99, 29])),
        ("red_sandstone_stairs", Rgb([186, 99, 29])),
        ("red_sandstone_wall", Rgb([186, 99, 29])),
        ("smooth_red_sandstone", Rgb([186, 99, 29])),
        // Nether blocks
        ("netherrack", Rgb([111, 54, 53])),
        ("nether_bricks", Rgb([44, 21, 26])),
        ("nether_brick_slab", Rgb([44, 21, 26])),
        ("nether_brick_stairs", Rgb([44, 21, 26])),
        ("nether_brick_wall", Rgb([44, 21, 26])),
        ("nether_brick_fence", Rgb([44, 21, 26])),
        ("red_nether_bricks", Rgb([69, 7, 9])),
        ("red_nether_brick_slab", Rgb([69, 7, 9])),
        ("red_nether_brick_stairs", Rgb([69, 7, 9])),
        ("red_nether_brick_wall", Rgb([69, 7, 9])),
        ("soul_sand", Rgb([81, 62, 51])),
        ("soul_soil", Rgb([75, 57, 46])),
        ("basalt", Rgb([73, 72, 77])),
        ("polished_basalt", Rgb([88, 87, 91])),
        ("smooth_basalt", Rgb([72, 72, 78])),
        ("blackstone", Rgb([42, 36, 41])),
        ("blackstone_slab", Rgb([42, 36, 41])),
        ("blackstone_stairs", Rgb([42, 36, 41])),
        ("blackstone_wall", Rgb([42, 36, 41])),
        ("polished_blackstone", Rgb([53, 49, 56])),
        ("polished_blackstone_bricks", Rgb([48, 43, 50])),
        ("polished_blackstone_brick_slab", Rgb([48, 43, 50])),
        ("polished_blackstone_brick_stairs", Rgb([48, 43, 50])),
        ("polished_blackstone_brick_wall", Rgb([48, 43, 50])),
        ("glowstone", Rgb([171, 131, 84])),
        ("shroomlight", Rgb([240, 146, 70])),
        ("crying_obsidian", Rgb([32, 10, 60])),
        ("obsidian", Rgb([15, 10, 24])),
        // End blocks
        ("end_stone", Rgb([219, 222, 158])),
        ("end_stone_bricks", Rgb([218, 224, 162])),
        ("end_stone_brick_slab", Rgb([218, 224, 162])),
        ("end_stone_brick_stairs", Rgb([218, 224, 162])),
        ("end_stone_brick_wall", Rgb([218, 224, 162])),
        ("purpur_block", Rgb([170, 126, 170])),
        ("purpur_pillar", Rgb([171, 129, 171])),
        ("purpur_slab", Rgb([170, 126, 170])),
        ("purpur_stairs", Rgb([170, 126, 170])),
        // Ores and minerals
        ("coal_ore", Rgb([105, 105, 105])),
        ("iron_ore", Rgb([136, 130, 127])),
        ("copper_ore", Rgb([124, 125, 120])),
        ("gold_ore", Rgb([143, 140, 125])),
        ("redstone_ore", Rgb([133, 107, 107])),
        ("emerald_ore", Rgb([108, 136, 115])),
        ("lapis_ore", Rgb([99, 112, 135])),
        ("diamond_ore", Rgb([121, 141, 140])),
        ("coal_block", Rgb([16, 15, 15])),
        ("iron_block", Rgb([220, 220, 220])),
        ("copper_block", Rgb([192, 107, 79])),
        ("gold_block", Rgb([246, 208, 62])),
        ("redstone_block", Rgb([170, 0, 0])),
        ("emerald_block", Rgb([42, 203, 88])),
        ("lapis_block", Rgb([38, 67, 156])),
        ("diamond_block", Rgb([97, 219, 213])),
        ("netherite_block", Rgb([66, 61, 63])),
        ("amethyst_block", Rgb([133, 97, 191])),
        ("raw_iron_block", Rgb([166, 136, 107])),
        ("raw_copper_block", Rgb([154, 105, 79])),
        ("raw_gold_block", Rgb([221, 169, 46])),
        // Quartz
        ("quartz_block", Rgb([235, 229, 222])),
        ("quartz_slab", Rgb([235, 229, 222])),
        ("quartz_stairs", Rgb([235, 229, 222])),
        ("smooth_quartz", Rgb([235, 229, 222])),
        ("smooth_quartz_slab", Rgb([235, 229, 222])),
        ("smooth_quartz_stairs", Rgb([235, 229, 222])),
        ("quartz_bricks", Rgb([234, 229, 221])),
        ("quartz_pillar", Rgb([235, 230, 224])),
        ("chiseled_quartz_block", Rgb([231, 226, 218])),
        // Prismarine
        ("prismarine", Rgb([76, 128, 113])),
        ("prismarine_slab", Rgb([76, 128, 113])),
        ("prismarine_stairs", Rgb([76, 128, 113])),
        ("prismarine_wall", Rgb([76, 128, 113])),
        ("prismarine_bricks", Rgb([99, 172, 158])),
        ("prismarine_brick_slab", Rgb([99, 172, 158])),
        ("prismarine_brick_stairs", Rgb([99, 172, 158])),
        ("dark_prismarine", Rgb([51, 91, 75])),
        ("dark_prismarine_slab", Rgb([51, 91, 75])),
        ("dark_prismarine_stairs", Rgb([51, 91, 75])),
        ("sea_lantern", Rgb([172, 199, 190])),
        // Copper variants
        ("exposed_copper", Rgb([161, 125, 103])),
        ("weathered_copper", Rgb([109, 145, 107])),
        ("oxidized_copper", Rgb([82, 162, 132])),
        ("cut_copper", Rgb([191, 106, 80])),
        ("cut_copper_slab", Rgb([191, 106, 80])),
        ("cut_copper_stairs", Rgb([191, 106, 80])),
        ("exposed_cut_copper", Rgb([154, 121, 101])),
        ("exposed_cut_copper_slab", Rgb([154, 121, 101])),
        ("exposed_cut_copper_stairs", Rgb([154, 121, 101])),
        ("weathered_cut_copper", Rgb([109, 145, 107])),
        ("weathered_cut_copper_slab", Rgb([109, 145, 107])),
        ("weathered_cut_copper_stairs", Rgb([109, 145, 107])),
        ("oxidized_cut_copper", Rgb([79, 153, 126])),
        ("oxidized_cut_copper_slab", Rgb([79, 153, 126])),
        ("oxidized_cut_copper_stairs", Rgb([79, 153, 126])),
        // Glass (slightly transparent look)
        ("glass", Rgb([200, 220, 230])),
        ("glass_pane", Rgb([200, 220, 230])),
        ("white_stained_glass", Rgb([255, 255, 255])),
        ("white_stained_glass_pane", Rgb([255, 255, 255])),
        ("orange_stained_glass", Rgb([216, 127, 51])),
        ("orange_stained_glass_pane", Rgb([216, 127, 51])),
        ("magenta_stained_glass", Rgb([178, 76, 216])),
        ("magenta_stained_glass_pane", Rgb([178, 76, 216])),
        ("light_blue_stained_glass", Rgb([102, 153, 216])),
        ("light_blue_stained_glass_pane", Rgb([102, 153, 216])),
        ("yellow_stained_glass", Rgb([229, 229, 51])),
        ("yellow_stained_glass_pane", Rgb([229, 229, 51])),
        ("lime_stained_glass", Rgb([127, 204, 25])),
        ("lime_stained_glass_pane", Rgb([127, 204, 25])),
        ("pink_stained_glass", Rgb([242, 127, 165])),
        ("pink_stained_glass_pane", Rgb([242, 127, 165])),
        ("gray_stained_glass", Rgb([76, 76, 76])),
        ("gray_stained_glass_pane", Rgb([76, 76, 76])),
        ("light_gray_stained_glass", Rgb([153, 153, 153])),
        ("light_gray_stained_glass_pane", Rgb([153, 153, 153])),
        ("cyan_stained_glass", Rgb([76, 127, 153])),
        ("cyan_stained_glass_pane", Rgb([76, 127, 153])),
        ("purple_stained_glass", Rgb([127, 63, 178])),
        ("purple_stained_glass_pane", Rgb([127, 63, 178])),
        ("blue_stained_glass", Rgb([51, 76, 178])),
        ("blue_stained_glass_pane", Rgb([51, 76, 178])),
        ("brown_stained_glass", Rgb([102, 76, 51])),
        ("brown_stained_glass_pane", Rgb([102, 76, 51])),
        ("green_stained_glass", Rgb([102, 127, 51])),
        ("green_stained_glass_pane", Rgb([102, 127, 51])),
        ("red_stained_glass", Rgb([153, 51, 51])),
        ("red_stained_glass_pane", Rgb([153, 51, 51])),
        ("black_stained_glass", Rgb([25, 25, 25])),
        ("black_stained_glass_pane", Rgb([25, 25, 25])),
        // Misc common blocks
        ("bookshelf", Rgb([116, 89, 53])),
        ("hay_block", Rgb([166, 139, 12])),
        ("melon", Rgb([111, 145, 31])),
        ("pumpkin", Rgb([198, 118, 24])),
        ("jack_o_lantern", Rgb([213, 139, 42])),
        ("carved_pumpkin", Rgb([198, 118, 24])),
        ("tnt", Rgb([219, 68, 52])),
        ("sponge", Rgb([195, 192, 74])),
        ("wet_sponge", Rgb([171, 181, 70])),
        ("moss_block", Rgb([89, 109, 45])),
        ("moss_carpet", Rgb([89, 109, 45])),
        ("sculk", Rgb([12, 28, 36])),
        ("honeycomb_block", Rgb([229, 148, 29])),
        ("slime_block", Rgb([111, 192, 91])),
        ("honey_block", Rgb([251, 185, 52])),
        ("barrel", Rgb([140, 106, 60])),
        ("chest", Rgb([155, 113, 48])),
        ("trapped_chest", Rgb([155, 113, 48])),
        ("crafting_table", Rgb([144, 109, 67])),
        ("furnace", Rgb([110, 110, 110])),
        ("blast_furnace", Rgb([80, 80, 85])),
        ("smoker", Rgb([90, 80, 70])),
        ("anvil", Rgb([68, 68, 68])),
        ("lectern", Rgb([180, 140, 90])),
        ("composter", Rgb([100, 80, 45])),
        ("cauldron", Rgb([60, 60, 60])),
        ("hopper", Rgb([70, 70, 70])),
        ("jukebox", Rgb([130, 90, 70])),
        ("note_block", Rgb([120, 80, 65])),
        ("bell", Rgb([200, 170, 50])),
        // Roads and paths
        ("dirt_path", Rgb([148, 121, 65])),
        ("farmland", Rgb([143, 88, 46])),
        ("mycelium", Rgb([111, 99, 107])),
        // Rails and redstone
        ("rail", Rgb([125, 108, 77])),
        ("powered_rail", Rgb([153, 126, 55])),
        ("detector_rail", Rgb([120, 97, 80])),
        ("activator_rail", Rgb([117, 85, 76])),
        ("redstone_wire", Rgb([170, 0, 0])),
        ("redstone_torch", Rgb([170, 0, 0])),
        ("redstone_lamp", Rgb([180, 130, 70])),
        ("lever", Rgb([100, 80, 60])),
        ("tripwire_hook", Rgb([120, 100, 80])),
        // Lighting
        ("torch", Rgb([255, 200, 100])),
        ("wall_torch", Rgb([255, 200, 100])),
        ("lantern", Rgb([200, 150, 80])),
        ("soul_lantern", Rgb([80, 200, 200])),
        ("soul_torch", Rgb([80, 200, 200])),
        ("soul_wall_torch", Rgb([80, 200, 200])),
        ("campfire", Rgb([200, 100, 50])),
        ("soul_campfire", Rgb([80, 200, 200])),
        ("candle", Rgb([200, 180, 130])),
        // Flowers and plants
        ("dandelion", Rgb([255, 236, 85])),
        ("poppy", Rgb([200, 30, 30])),
        ("blue_orchid", Rgb([47, 186, 199])),
        ("allium", Rgb([190, 130, 200])),
        ("azure_bluet", Rgb([220, 230, 220])),
        ("red_tulip", Rgb([200, 50, 50])),
        ("orange_tulip", Rgb([230, 130, 50])),
        ("white_tulip", Rgb([230, 230, 220])),
        ("pink_tulip", Rgb([220, 150, 170])),
        ("oxeye_daisy", Rgb([230, 230, 200])),
        ("cornflower", Rgb([70, 90, 180])),
        ("lily_of_the_valley", Rgb([230, 230, 230])),
        ("wither_rose", Rgb([30, 30, 30])),
        ("sunflower", Rgb([255, 200, 50])),
        ("lilac", Rgb([200, 150, 200])),
        ("rose_bush", Rgb([180, 40, 40])),
        ("peony", Rgb([230, 180, 200])),
        ("fern", Rgb([80, 120, 60])),
        ("large_fern", Rgb([80, 120, 60])),
        ("dead_bush", Rgb([150, 120, 80])),
        ("seagrass", Rgb([40, 100, 60])),
        ("tall_seagrass", Rgb([40, 100, 60])),
        ("kelp", Rgb([50, 110, 60])),
        ("kelp_plant", Rgb([50, 110, 60])),
        ("sugar_cane", Rgb([140, 180, 100])),
        ("bamboo", Rgb([90, 140, 50])),
        ("vine", Rgb([50, 100, 40])),
        ("lily_pad", Rgb([40, 110, 40])),
        ("sweet_berry_bush", Rgb([60, 90, 50])),
        ("cactus", Rgb([85, 127, 52])),
        // Carpets
        ("white_carpet", Rgb([234, 236, 237])),
        ("orange_carpet", Rgb([241, 118, 20])),
        ("magenta_carpet", Rgb([190, 68, 179])),
        ("light_blue_carpet", Rgb([58, 175, 217])),
        ("yellow_carpet", Rgb([249, 198, 40])),
        ("lime_carpet", Rgb([112, 185, 26])),
        ("pink_carpet", Rgb([238, 141, 172])),
        ("gray_carpet", Rgb([63, 68, 72])),
        ("light_gray_carpet", Rgb([142, 142, 135])),
        ("cyan_carpet", Rgb([21, 138, 145])),
        ("purple_carpet", Rgb([122, 42, 173])),
        ("blue_carpet", Rgb([53, 57, 157])),
        ("brown_carpet", Rgb([114, 72, 41])),
        ("green_carpet", Rgb([85, 110, 28])),
        ("red_carpet", Rgb([161, 39, 35])),
        ("black_carpet", Rgb([21, 21, 26])),
        // Signs and banners
        ("oak_sign", Rgb([162, 130, 78])),
        ("oak_wall_sign", Rgb([162, 130, 78])),
        ("spruce_sign", Rgb([115, 85, 49])),
        ("spruce_wall_sign", Rgb([115, 85, 49])),
        ("birch_sign", Rgb([196, 179, 123])),
        ("birch_wall_sign", Rgb([196, 179, 123])),
        ("dark_oak_sign", Rgb([67, 43, 20])),
        ("dark_oak_wall_sign", Rgb([67, 43, 20])),
        // Beds (show blanket color from top)
        ("white_bed", Rgb([234, 236, 237])),
        ("orange_bed", Rgb([241, 118, 20])),
        ("magenta_bed", Rgb([190, 68, 179])),
        ("light_blue_bed", Rgb([58, 175, 217])),
        ("yellow_bed", Rgb([249, 198, 40])),
        ("lime_bed", Rgb([112, 185, 26])),
        ("pink_bed", Rgb([238, 141, 172])),
        ("gray_bed", Rgb([63, 68, 72])),
        ("light_gray_bed", Rgb([142, 142, 135])),
        ("cyan_bed", Rgb([21, 138, 145])),
        ("purple_bed", Rgb([122, 42, 173])),
        ("blue_bed", Rgb([53, 57, 157])),
        ("brown_bed", Rgb([114, 72, 41])),
        ("green_bed", Rgb([85, 110, 28])),
        ("red_bed", Rgb([161, 39, 35])),
        ("black_bed", Rgb([21, 21, 26])),
        // Trapdoors
        ("oak_trapdoor", Rgb([162, 130, 78])),
        ("spruce_trapdoor", Rgb([115, 85, 49])),
        ("birch_trapdoor", Rgb([196, 179, 123])),
        ("dark_oak_trapdoor", Rgb([67, 43, 20])),
        ("iron_trapdoor", Rgb([200, 200, 200])),
        // Fences and walls
        ("iron_bars", Rgb([150, 150, 150])),
        // Ladders
        ("ladder", Rgb([160, 130, 70])),
        // Crops
        ("wheat", Rgb([200, 180, 80])),
        ("carrots", Rgb([230, 140, 30])),
        ("potatoes", Rgb([180, 160, 80])),
        ("beetroots", Rgb([150, 50, 50])),
        ("pumpkin_stem", Rgb([120, 140, 70])),
        ("melon_stem", Rgb([120, 140, 70])),
    ])
}
