// Top-down map preview rendered from the in-memory world during save.

use crate::block_definitions::Block;
use crate::coordinate_system::cartesian::XZBBox;
use crate::world_editor::{BlockStorage, RegionToModify, SectionToModify, MAX_BLOCK_ID};
use fnv::FnvHashMap;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{Rgb, RgbImage};
use once_cell::sync::Lazy;
use std::cmp::Reverse;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Longest allowed output image side; larger worlds are box-averaged down.
const MAX_OUTPUT_SIDE: u32 = 4096;

/// Per-block-id color; None = transparent (top-block search looks below).
static COLOR_LUT: Lazy<Vec<Option<Rgb<u8>>>> = Lazy::new(build_color_lut);

fn build_color_lut() -> Vec<Option<Rgb<u8>>> {
    let colors = get_block_colors();
    (0..MAX_BLOCK_ID as u16)
        .map(|id| {
            let block = Block::from_raw_id(id);
            let name = block.try_name()?;
            if is_transparent_block(name) {
                return None;
            }
            Some(
                colors
                    .get(name)
                    .copied()
                    .unwrap_or_else(|| get_fallback_color(name)),
            )
        })
        .collect()
}

#[inline]
fn lut_color(block: Block) -> Option<Rgb<u8>> {
    COLOR_LUT.get(block.id() as usize).copied().flatten()
}

/// Collects top-block colors per region during save, averaged into the preview PNG.
pub struct PreviewAccumulator {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    /// Blocks per output pixel along each axis.
    step: u32,
    /// Column sampling interval per box; caps samples at 256 so u16 sums can't overflow.
    stride: u32,
    out_w: u32,
    out_h: u32,
    /// Per output pixel: [r_sum, g_sum, b_sum, sample_count].
    frame: Mutex<Vec<[u16; 4]>>,
}

impl PreviewAccumulator {
    pub fn new(xzbbox: &XZBBox) -> Self {
        Self::new_capped(xzbbox, MAX_OUTPUT_SIDE)
    }

    // Smaller caps keep the frame cheap when only a low-res consumer (map item) needs it.
    pub fn new_capped(xzbbox: &XZBBox, max_side: u32) -> Self {
        let width = (xzbbox.max_x() - xzbbox.min_x() + 1) as u32;
        let height = (xzbbox.max_z() - xzbbox.min_z() + 1) as u32;
        let step = width.max(height).div_ceil(max_side.max(1)).max(1);
        let out_w = width.div_ceil(step);
        let out_h = height.div_ceil(step);
        Self {
            min_x: xzbbox.min_x(),
            max_x: xzbbox.max_x(),
            min_z: xzbbox.min_z(),
            max_z: xzbbox.max_z(),
            step,
            stride: step.div_ceil(16),
            out_w,
            out_h,
            frame: Mutex::new(vec![[0u16; 4]; out_w as usize * out_h as usize]),
        }
    }

    /// Accumulates one region's top-block colors; halo columns outside the bounds are clipped.
    pub(crate) fn ingest_region(&self, region_x: i32, region_z: i32, region: &RegionToModify) {
        let base_x = region_x * 512;
        let base_z = region_z * 512;
        let x0 = base_x.max(self.min_x);
        let z0 = base_z.max(self.min_z);
        let x1 = (base_x + 511).min(self.max_x);
        let z1 = (base_z + 511).min(self.max_z);
        if x0 > x1 || z0 > z1 {
            return;
        }
        // Local partial for this region's pixel window, merged under one short lock at the end.
        let px0 = (x0 - self.min_x) as u32 / self.step;
        let pz0 = (z0 - self.min_z) as u32 / self.step;
        let pw = ((x1 - self.min_x) as u32 / self.step - px0 + 1) as usize;
        let ph = ((z1 - self.min_z) as u32 / self.step - pz0 + 1) as usize;
        let mut local = vec![[0u16; 4]; pw * ph];

        for (&(chunk_x, chunk_z), chunk) in &region.chunks {
            let cbx = base_x + chunk_x * 16;
            let cbz = base_z + chunk_z * 16;
            if cbx + 15 < x0 || cbx > x1 || cbz + 15 < z0 || cbz > z1 {
                continue;
            }
            let mut sections: Vec<(i8, &SectionToModify)> =
                chunk.sections.iter().map(|(y, s)| (*y, s)).collect();
            if sections.is_empty() {
                continue;
            }
            sections.sort_unstable_by_key(|(y, _)| Reverse(*y));
            for lz in 0..16 {
                let wz = cbz + lz;
                if wz < z0
                    || wz > z1
                    || !((wz - self.min_z) as u32 % self.step).is_multiple_of(self.stride)
                {
                    continue;
                }
                for lx in 0..16 {
                    let wx = cbx + lx;
                    if wx < x0
                        || wx > x1
                        || !((wx - self.min_x) as u32 % self.step).is_multiple_of(self.stride)
                    {
                        continue;
                    }
                    if let Some((color, y)) = top_block_color(&sections, lx as u8, lz as u8) {
                        let c = apply_elevation_shading(color, y);
                        let px = (wx - self.min_x) as u32 / self.step;
                        let pz = (wz - self.min_z) as u32 / self.step;
                        let cell = &mut local[(pz - pz0) as usize * pw + (px - px0) as usize];
                        cell[0] += u16::from(c.0[0]);
                        cell[1] += u16::from(c.0[1]);
                        cell[2] += u16::from(c.0[2]);
                        cell[3] += 1;
                    }
                }
            }
        }

        let mut frame = self.frame.lock().unwrap();
        for row in 0..ph {
            let out_base = (pz0 as usize + row) * self.out_w as usize + px0 as usize;
            for (col, src) in local[row * pw..(row + 1) * pw].iter().enumerate() {
                if src[3] == 0 {
                    continue;
                }
                let dst = &mut frame[out_base + col];
                dst[0] += src[0];
                dst[1] += src[1];
                dst[2] += src[2];
                dst[3] += src[3];
            }
        }
    }

    pub fn min_x(&self) -> i32 {
        self.min_x
    }

    pub fn min_z(&self) -> i32 {
        self.min_z
    }

    /// Blocks per preview pixel.
    pub fn step(&self) -> u32 {
        self.step
    }

    /// Averages the samples into an image without consuming the frame.
    pub fn render_image(&self) -> RgbImage {
        let frame = self.frame.lock().unwrap();
        let mut img = RgbImage::from_pixel(self.out_w, self.out_h, Rgb([255, 255, 255]));
        for (i, cell) in frame.iter().enumerate() {
            let n = cell[3];
            if n == 0 {
                continue;
            }
            let px = Rgb([
                (cell[0] / n) as u8,
                (cell[1] / n) as u8,
                (cell[2] / n) as u8,
            ]);
            img.put_pixel(i as u32 % self.out_w, i as u32 / self.out_w, px);
        }
        img
    }

    /// Averages the samples and writes the PNG; unsampled pixels stay white.
    pub fn finalize(&self, output_path: &Path) -> Result<PathBuf, String> {
        let img = self.render_image();
        // Free the accumulator; the preview is only written once.
        drop(std::mem::take(&mut *self.frame.lock().unwrap()));

        let file = std::fs::File::create(output_path)
            .map_err(|e| format!("Failed to create map image: {e}"))?;
        let encoder = PngEncoder::new_with_quality(
            BufWriter::new(file),
            CompressionType::Default,
            FilterType::Adaptive,
        );
        img.write_with_encoder(encoder)
            .map_err(|e| format!("Failed to save map image: {e}"))?;
        Ok(output_path.to_path_buf())
    }
}

/// Topmost non-transparent block color and world Y, sections pre-sorted by Y descending.
fn top_block_color(sections: &[(i8, &SectionToModify)], x: u8, z: u8) -> Option<(Rgb<u8>, i32)> {
    for (section_y, section) in sections {
        match &section.storage {
            BlockStorage::Uniform(b) => {
                if let Some(c) = lut_color(*b) {
                    return Some((c, i32::from(*section_y) * 16 + 15));
                }
            }
            _ => {
                for y in (0..16u8).rev() {
                    let b = section.storage.get(SectionToModify::index(x, y, z));
                    if let Some(c) = lut_color(b) {
                        return Some((c, i32::from(*section_y) * 16 + i32::from(y)));
                    }
                }
            }
        }
    }
    None
}

/// Applies elevation-based shading to a color
/// Higher elevations are brighter, lower are darker
#[inline]
fn apply_elevation_shading(color: Rgb<u8>, y: i32) -> Rgb<u8> {
    // Base brightness boost of 10%, plus elevation shading
    // Shading range: -20% darker to +20% brighter (asymmetric, more bright than dark)

    // Normalize Y to a -1.0 to 1.0 range (roughly)
    // y=0 -> -0.5, y=0 -> 0, y=200 -> +1.0
    let normalized = (y as f32 / 100.0).clamp(-1.0, 1.0);

    // Base 10% brightness boost + asymmetric elevation shading
    let elevation_adjust = if normalized >= 0.0 {
        // Above sea level: up to +20% brighter
        normalized * 0.20
    } else {
        // Below sea level: up to -20% darker
        normalized * 0.20
    };

    let multiplier = 1.10 + elevation_adjust;

    Rgb([
        (color.0[0] as f32 * multiplier).clamp(0.0, 255.0) as u8,
        (color.0[1] as f32 * multiplier).clamp(0.0, 255.0) as u8,
        (color.0[2] as f32 * multiplier).clamp(0.0, 255.0) as u8,
    ])
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
fn get_block_colors() -> FnvHashMap<&'static str, Rgb<u8>> {
    FnvHashMap::from_iter([
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
        ("water", Rgb([59, 86, 165])),
        ("ice", Rgb([145, 183, 253])),
        ("packed_ice", Rgb([141, 180, 250])),
        ("blue_ice", Rgb([116, 167, 253])),
        ("snow", Rgb([249, 254, 254])),
        ("snow_block", Rgb([249, 254, 254])),
        ("powder_snow", Rgb([248, 253, 253])),
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
        ("oak_leaves", Rgb([55, 95, 36])),
        ("spruce_leaves", Rgb([61, 99, 61])),
        ("birch_leaves", Rgb([80, 106, 47])),
        ("jungle_leaves", Rgb([48, 113, 20])),
        ("acacia_leaves", Rgb([75, 104, 40])),
        ("dark_oak_leaves", Rgb([35, 82, 11])),
        ("mangrove_leaves", Rgb([69, 123, 38])),
        ("cherry_leaves", Rgb([228, 177, 197])),
        ("azalea_leaves", Rgb([71, 96, 37])),
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
        ("end_stone", Rgb([219, 222, 158])),
        ("end_stone_bricks", Rgb([218, 224, 162])),
        ("end_stone_brick_slab", Rgb([218, 224, 162])),
        ("end_stone_brick_stairs", Rgb([218, 224, 162])),
        ("end_stone_brick_wall", Rgb([218, 224, 162])),
        ("purpur_block", Rgb([170, 126, 170])),
        ("purpur_pillar", Rgb([171, 129, 171])),
        ("purpur_slab", Rgb([170, 126, 170])),
        ("purpur_stairs", Rgb([170, 126, 170])),
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
        ("quartz_block", Rgb([235, 229, 222])),
        ("quartz_slab", Rgb([235, 229, 222])),
        ("quartz_stairs", Rgb([235, 229, 222])),
        ("smooth_quartz", Rgb([235, 229, 222])),
        ("smooth_quartz_slab", Rgb([235, 229, 222])),
        ("smooth_quartz_stairs", Rgb([235, 229, 222])),
        ("quartz_bricks", Rgb([234, 229, 221])),
        ("quartz_pillar", Rgb([235, 230, 224])),
        ("chiseled_quartz_block", Rgb([231, 226, 218])),
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
        ("dirt_path", Rgb([148, 121, 65])),
        ("farmland", Rgb([143, 88, 46])),
        ("mycelium", Rgb([111, 99, 107])),
        ("rail", Rgb([125, 108, 77])),
        ("powered_rail", Rgb([153, 126, 55])),
        ("detector_rail", Rgb([120, 97, 80])),
        ("activator_rail", Rgb([117, 85, 76])),
        ("redstone_wire", Rgb([170, 0, 0])),
        ("redstone_torch", Rgb([170, 0, 0])),
        ("redstone_lamp", Rgb([180, 130, 70])),
        ("lever", Rgb([100, 80, 60])),
        ("tripwire_hook", Rgb([120, 100, 80])),
        ("torch", Rgb([255, 200, 100])),
        ("wall_torch", Rgb([255, 200, 100])),
        ("lantern", Rgb([200, 150, 80])),
        ("soul_lantern", Rgb([80, 200, 200])),
        ("soul_torch", Rgb([80, 200, 200])),
        ("soul_wall_torch", Rgb([80, 200, 200])),
        ("campfire", Rgb([200, 100, 50])),
        ("soul_campfire", Rgb([80, 200, 200])),
        ("candle", Rgb([200, 180, 130])),
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
        ("oak_sign", Rgb([162, 130, 78])),
        ("oak_wall_sign", Rgb([162, 130, 78])),
        ("spruce_sign", Rgb([115, 85, 49])),
        ("spruce_wall_sign", Rgb([115, 85, 49])),
        ("birch_sign", Rgb([196, 179, 123])),
        ("birch_wall_sign", Rgb([196, 179, 123])),
        ("dark_oak_sign", Rgb([67, 43, 20])),
        ("dark_oak_wall_sign", Rgb([67, 43, 20])),
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
        ("oak_trapdoor", Rgb([162, 130, 78])),
        ("spruce_trapdoor", Rgb([115, 85, 49])),
        ("birch_trapdoor", Rgb([196, 179, 123])),
        ("dark_oak_trapdoor", Rgb([67, 43, 20])),
        ("iron_trapdoor", Rgb([200, 200, 200])),
        ("iron_bars", Rgb([150, 150, 150])),
        ("ladder", Rgb([160, 130, 70])),
        ("wheat", Rgb([200, 180, 80])),
        ("carrots", Rgb([230, 140, 30])),
        ("potatoes", Rgb([180, 160, 80])),
        ("beetroots", Rgb([150, 50, 50])),
        ("pumpkin_stem", Rgb([120, 140, 70])),
        ("melon_stem", Rgb([120, 140, 70])),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{GLASS, GRASS_BLOCK, STONE, WATER};
    use crate::world_editor::RegionToModify;

    fn accum(min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> PreviewAccumulator {
        let bbox = XZBBox::rect_from_min_max(min_x, min_z, max_x, max_z).unwrap();
        PreviewAccumulator::new(&bbox)
    }

    #[test]
    fn downscale_caps_output_side() {
        // ~500 km2 world: 22400 blocks per side -> step 6, bounded output.
        let a = accum(0, 0, 22399, 22399);
        assert_eq!(a.step, 6);
        assert_eq!(a.stride, 1);
        assert_eq!(a.out_w, 3734);
        assert_eq!(a.out_h, 3734);

        let b = accum(0, 0, 999, 499);
        assert_eq!(b.step, 1);
        assert_eq!((b.out_w, b.out_h), (1000, 500));

        // Beyond step 16 columns are strided so u16 sums cannot overflow.
        let c = accum(0, 0, 69999, 69999);
        assert_eq!(c.step, 18);
        assert_eq!(c.stride, 2);
    }

    #[test]
    fn transparent_lut_and_colors() {
        assert!(lut_color(GLASS).is_none());
        assert_eq!(lut_color(STONE), Some(Rgb([128, 128, 128])));
        assert_eq!(lut_color(WATER), Some(Rgb([59, 86, 165])));
    }

    #[test]
    fn renders_top_block_skips_transparent_and_averages() {
        let a = accum(0, 0, 15, 15);
        let mut region = RegionToModify::default();
        let chunk = region.get_or_create_chunk(0, 0);
        // Column (0,0): stone at y=0.
        chunk.set_block(0, 0, 0, STONE);
        // Column (1,0): grass at y=5 hidden under glass at y=10 (transparent).
        chunk.set_block(1, 5, 0, GRASS_BLOCK);
        chunk.set_block(1, 10, 0, GLASS);
        a.ingest_region(0, 0, &region);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        a.finalize(&path).unwrap();
        let img = image::open(&path).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (16, 16));
        assert_eq!(
            *img.get_pixel(0, 0),
            apply_elevation_shading(Rgb([128, 128, 128]), 0)
        );
        assert_eq!(
            *img.get_pixel(1, 0),
            apply_elevation_shading(Rgb([86, 125, 70]), 5)
        );
        // Untouched column stays white.
        assert_eq!(*img.get_pixel(5, 5), Rgb([255, 255, 255]));
    }

    #[test]
    fn box_average_and_halo_clipping() {
        // 8192-wide world -> step 2: columns (0,0) and (1,0) share one pixel.
        let a = accum(0, 0, 8191, 15);
        assert_eq!(a.step, 2);
        let mut region = RegionToModify::default();
        let chunk = region.get_or_create_chunk(0, 0);
        chunk.set_block(0, 0, 0, STONE);
        chunk.set_block(1, 0, 0, WATER);
        a.ingest_region(0, 0, &region);
        // Halo region entirely outside the bbox must be clipped without panicking.
        let mut halo = RegionToModify::default();
        halo.get_or_create_chunk(0, 0).set_block(0, 0, 0, STONE);
        a.ingest_region(-1, -1, &halo);

        let s = apply_elevation_shading(Rgb([128, 128, 128]), 0);
        let w = apply_elevation_shading(Rgb([59, 86, 165]), 0);
        let expected = Rgb([
            ((s.0[0] as u32 + w.0[0] as u32) / 2) as u8,
            ((s.0[1] as u32 + w.0[1] as u32) / 2) as u8,
            ((s.0[2] as u32 + w.0[2] as u32) / 2) as u8,
        ]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        a.finalize(&path).unwrap();
        let img = image::open(&path).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (4096, 8));
        assert_eq!(*img.get_pixel(0, 0), expected);
        assert_eq!(*img.get_pixel(1, 0), Rgb([255, 255, 255]));
    }
}
