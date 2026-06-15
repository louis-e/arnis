//! Wikidata pre-scan + scale/color/orient resolution + placement.

use crate::args::Args;
use crate::block_definitions::*;
use crate::colors::{color_text_to_rgb_tuple, RGBTuple};
use crate::models_3d::palette::closest_blocks;
use crate::models_3d::voxelize::{
    glb_model_bbox, voxelize_glb, voxelize_uniform_triangles, WorldTransform,
};
use crate::models_3d::wikidata::client::fetch_model;
use crate::models_3d::wikidata::index::lookup;
use crate::models_3d::wikidata::stl::{bbox, parse_triangles};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

/// Top-K perceptually-close blocks when the OSM element has an explicit color/material tag.
const COLOR_PALETTE_K: usize = 5;

/// Final-extent caps in meters: tall structures pass; horizontally-massive ones don't.
const MAX_XZ_EXTENT_M: f32 = 225.0;
const MAX_Y_EXTENT_M: f32 = 600.0;
const MIN_EXTENT_M: f32 = 2.0;

#[derive(Clone, Debug)]
struct Placement {
    osm_id: u64,
    qid: String,
    anchor_x: i32,
    anchor_z: i32,
    footprint: Bbox,
    world_yaw_degrees: f64,
    /// Block pool sampled per-voxel for texture variation.
    palette: Vec<Block>,
    palette_layers: Option<ResolvedLayers>,
    /// Target height in meters; `None` → fall back to XZ-only scaling.
    osm_height_m: Option<f64>,
    /// Target XZ extent in meters; `None` → fall back to height-only scaling.
    osm_xz_extent_m: Option<f64>,
}

#[derive(Clone, Debug)]
struct ResolvedLayers {
    y_max_frac: Vec<f32>,
    blocks: Vec<Vec<Block>>,
}

#[derive(Clone, Copy, Debug)]
struct Bbox {
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
}

impl Bbox {
    fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }
}

pub struct PrescanResult {
    pub suppressed_ids: HashSet<(&'static str, u64)>,
    placements: Vec<Placement>,
    fetched: HashMap<String, Vec<u8>>,
}

impl PrescanResult {
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Regions each placement may write to (stream-to-disk deferral). XZ extent is capped
    /// at MAX_XZ_EXTENT_M, so anchor ± that radius (+ ring) is a safe superset.
    pub fn deferred_region_keys(&self, scale: f64) -> Vec<(i32, i32)> {
        let r = (MAX_XZ_EXTENT_M as f64 * scale).ceil() as i32;
        self.placements
            .iter()
            .flat_map(|p| crate::models_3d::region_keys_around(p.anchor_x, p.anchor_z, r))
            .collect()
    }
}

struct Candidate {
    placement: Placement,
    raw_footprint: Option<Bbox>,
    osm_kind: &'static str,
}

/// Identify candidates, fetch models, suppress only successful fetches.
pub fn prescan(
    elements: &[ProcessedElement],
    already_suppressed: &HashSet<(&'static str, u64)>,
    world_rotation: f64,
    args_scale: f64,
) -> PrescanResult {
    let mut candidates: Vec<Candidate> = Vec::new();

    for element in elements {
        if already_suppressed.contains(&(element.kind(), element.id())) {
            continue;
        }
        let Some(qid_raw) = element.tags().get("wikidata") else {
            continue;
        };
        let qid = qid_raw.trim();
        let Some(entry) = lookup(qid) else { continue };

        let Some((anchor_x, anchor_z)) = anchor_xz(element) else {
            continue;
        };

        // 1-point bbox (node) → no real footprint; fall back to synthetic 16×16.
        let raw_footprint = osm_bbox(element).filter(|b| b.max_x > b.min_x || b.max_z > b.min_z);
        let footprint = raw_footprint.unwrap_or_else(|| Bbox {
            min_x: anchor_x - 8,
            max_x: anchor_x + 8,
            min_z: anchor_z - 8,
            max_z: anchor_z + 8,
        });

        // Curated height_m wins; force uniform scaling so OSM tag/footprint can't squash the model.
        let osm_height_m = entry
            .height_m
            .or_else(|| element.tags().get("height").and_then(|s| parse_meters(s)));

        let osm_xz_extent_m = if entry.height_m.is_some() {
            None
        } else {
            let dx = (footprint.max_x - footprint.min_x) as f64;
            let dz = (footprint.max_z - footprint.min_z) as f64;
            Some(dx.max(dz) / args_scale)
        };

        let osm_yaw = element
            .tags()
            .get("direction")
            .and_then(|s| parse_direction(s))
            .unwrap_or(0.0);
        let world_yaw_degrees = osm_yaw + world_rotation;

        let palette = resolve_palette(element);
        let palette_layers = entry
            .palette_layers
            .as_deref()
            .and_then(resolve_palette_layers);

        candidates.push(Candidate {
            placement: Placement {
                osm_id: element.id(),
                qid: qid.to_string(),
                anchor_x,
                anchor_z,
                footprint,
                world_yaw_degrees,
                palette,
                palette_layers,
                osm_height_m,
                osm_xz_extent_m,
            },
            raw_footprint,
            osm_kind: element.kind(),
        });
    }

    let unique_urls: Vec<String> = {
        let mut set: HashSet<String> = HashSet::new();
        for c in &candidates {
            if let Some(e) = lookup(&c.placement.qid) {
                set.insert(e.url.clone());
            }
        }
        set.into_iter().collect()
    };
    let fetched: HashMap<String, Vec<u8>> = unique_urls
        .par_iter()
        .filter_map(|url| match fetch_model(url) {
            Ok(bytes) => Some((url.clone(), bytes)),
            Err(e) => {
                eprintln!(
                    "{} Wikidata model fetch failed ({url}): {e}",
                    "Warning:".yellow().bold()
                );
                None
            }
        })
        .collect();

    let mut suppressed: HashSet<(&'static str, u64)> = HashSet::new();
    let mut placements: Vec<Placement> = Vec::new();
    let mut footprints: Vec<Bbox> = Vec::new();
    for c in candidates {
        let Some(entry) = lookup(&c.placement.qid) else {
            continue;
        };
        if !fetched.contains_key(&entry.url) {
            continue;
        }
        suppressed.insert((c.osm_kind, c.placement.osm_id));
        if let Some(b) = c.raw_footprint {
            footprints.push(b);
        }
        placements.push(c.placement);
    }

    if !footprints.is_empty() {
        for element in elements {
            let key = (element.kind(), element.id());
            if suppressed.contains(&key) || already_suppressed.contains(&key) {
                continue;
            }
            let tags = element.tags();
            if !tags.contains_key("building") && !tags.contains_key("building:part") {
                continue;
            }
            let Some((cx, cz)) = anchor_xz(element) else {
                continue;
            };
            if footprints.iter().any(|b| b.contains(cx, cz)) {
                suppressed.insert(key);
            }
        }
    }

    PrescanResult {
        suppressed_ids: suppressed,
        placements,
        fetched,
    }
}

/// Voxelizes pre-fetched models and places blocks.
pub fn place_wikidata_models(editor: &mut WorldEditor, args: &Args, prescan: &PrescanResult) {
    if prescan.placements.is_empty() {
        return;
    }

    println!(
        "{} Placing {} Wikidata 3D model{}...",
        "  [+]".bold(),
        prescan.placements.len(),
        if prescan.placements.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    let mut placed = 0usize;
    let mut total_voxels = 0usize;

    for placement in &prescan.placements {
        let Some(entry) = lookup(&placement.qid) else {
            continue;
        };
        let Some(bytes) = prescan.fetched.get(&entry.url) else {
            continue;
        };

        let fp = &placement.footprint;
        let ground_y =
            crate::models_3d::lowest_ground_in_bbox(editor, fp.min_x, fp.min_z, fp.max_x, fp.max_z);

        let mut voxels = if is_glb(bytes) {
            match voxelize_glb_for_placement(bytes, placement, args, ground_y) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "{} Wikidata GLB voxelization failed ({}, OSM {}): {e}",
                        "Warning:".yellow().bold(),
                        placement.qid,
                        placement.osm_id
                    );
                    continue;
                }
            }
        } else {
            match voxelize_stl(bytes, placement, args, ground_y) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "{} Wikidata STL voxelization failed ({}, OSM {}): {e}",
                        "Warning:".yellow().bold(),
                        placement.qid,
                        placement.osm_id
                    );
                    continue;
                }
            }
        };

        if voxels.is_empty() {
            eprintln!(
                "{} Wikidata model produced no voxels ({}, OSM {})",
                "Warning:".yellow().bold(),
                placement.qid,
                placement.osm_id
            );
            continue;
        }

        let min_voxel_y = voxels.iter().map(|(p, _)| p[1]).min().unwrap();
        let dy = ground_y - min_voxel_y;
        if dy != 0 {
            for (pos, _) in voxels.iter_mut() {
                pos[1] += dy;
            }
        }

        for ([x, y, z], block) in &voxels {
            editor.set_block_absolute(*block, *x, *y, *z, None, None);
        }
        total_voxels += voxels.len();
        placed += 1;

        let artist = entry.artist.as_deref().unwrap_or("Wikimedia contributor");
        println!(
            "    Wikidata {} — \"{}\" by {} ({})",
            placement.qid, entry.label, artist, entry.license
        );
    }

    println!(
        "  Placed {} Wikidata model{} ({} blocks)",
        placed.to_string().bright_white().bold(),
        if placed == 1 { "" } else { "s" },
        total_voxels
    );
}

// Per-category block pools modeled after `buildings::*_WALL_OPTIONS`.
const TOWER_PALETTE: &[Block] = &[
    STONE_BRICKS,
    COBBLESTONE,
    CRACKED_STONE_BRICKS,
    POLISHED_ANDESITE,
    ANDESITE,
    DEEPSLATE_BRICKS,
    SMOOTH_STONE,
    CHISELED_STONE_BRICKS,
];

const HISTORIC_PALETTE: &[Block] = &[
    STONE_BRICKS,
    CRACKED_STONE_BRICKS,
    CHISELED_STONE_BRICKS,
    COBBLESTONE,
    POLISHED_BLACKSTONE_BRICKS,
    MOSSY_STONE_BRICKS,
    MOSSY_COBBLESTONE,
    COBBLED_DEEPSLATE,
    ANDESITE,
    DEEPSLATE_BRICKS,
];

const RELIGIOUS_PALETTE: &[Block] = &[
    STONE_BRICKS,
    CHISELED_STONE_BRICKS,
    QUARTZ_BLOCK,
    WHITE_CONCRETE,
    SANDSTONE,
    SMOOTH_SANDSTONE,
    POLISHED_DIORITE,
    END_STONE_BRICKS,
];

const STATUE_PALETTE: &[Block] = &[
    STONE,
    SMOOTH_STONE,
    ANDESITE,
    POLISHED_ANDESITE,
    DIORITE,
    POLISHED_DIORITE,
];

const RESIDENTIAL_PALETTE: &[Block] = &[
    BRICK,
    STONE_BRICKS,
    OAK_PLANKS,
    MUD_BRICKS,
    SANDSTONE,
    TERRACOTTA,
    BROWN_TERRACOTTA,
];

const INDUSTRIAL_PALETTE: &[Block] = &[
    GRAY_CONCRETE,
    LIGHT_GRAY_CONCRETE,
    STONE,
    SMOOTH_STONE,
    POLISHED_ANDESITE,
    DEEPSLATE_BRICKS,
];

const LIGHTHOUSE_PALETTE: &[Block] = &[
    WHITE_CONCRETE,
    QUARTZ_BLOCK,
    SMOOTH_QUARTZ,
    POLISHED_DIORITE,
];

const DEFAULT_PALETTE: &[Block] = &[
    STONE_BRICKS,
    ANDESITE,
    POLISHED_ANDESITE,
    COBBLESTONE,
    SMOOTH_STONE,
];

/// Per-voxel palette pool: explicit color/material → structure-type category → fallback.
fn resolve_palette(element: &ProcessedElement) -> Vec<Block> {
    let tags = element.tags();
    if let Some(c) = tags
        .get("building:colour")
        .or_else(|| tags.get("colour"))
        .or_else(|| tags.get("color"))
    {
        if let Some(rgb) = color_text_to_rgb_tuple(c) {
            return closest_blocks(rgb, COLOR_PALETTE_K);
        }
    }
    if let Some(m) = tags.get("building:material") {
        if let Some(rgb) = material_to_rgb(m) {
            return closest_blocks(rgb, COLOR_PALETTE_K);
        }
    }
    structure_type_palette(tags).to_vec()
}

fn material_to_rgb(material: &str) -> Option<RGBTuple> {
    match material.to_ascii_lowercase().as_str() {
        "brick" => Some((151, 98, 83)),
        "stone" => Some((132, 135, 134)),
        "sandstone" => Some((216, 203, 156)),
        "concrete" => Some((128, 127, 128)),
        "wood" | "timber" => Some((162, 131, 79)),
        "marble" => Some((230, 226, 220)),
        "granite" => Some((149, 103, 86)),
        "limestone" => Some((210, 195, 165)),
        "metal" | "steel" | "iron" => Some((180, 180, 180)),
        "glass" => Some((180, 200, 215)),
        "copper" => Some((192, 108, 80)),
        _ => None,
    }
}

fn structure_type_palette(tags: &std::collections::HashMap<String, String>) -> &'static [Block] {
    if let Some(v) = tags.get("man_made") {
        match v.as_str() {
            "tower" | "obelisk" | "chimney" => return TOWER_PALETTE,
            "lighthouse" => return LIGHTHOUSE_PALETTE,
            "monument" => return STATUE_PALETTE,
            _ => {}
        }
    }
    if let Some(v) = tags.get("historic") {
        match v.as_str() {
            "castle" | "fort" | "ruins" | "city_gate" => return HISTORIC_PALETTE,
            "memorial" | "monument" => return STATUE_PALETTE,
            "archaeological_site" => return HISTORIC_PALETTE,
            _ => {}
        }
    }
    if let Some(v) = tags.get("amenity") {
        match v.as_str() {
            "place_of_worship" => return RELIGIOUS_PALETTE,
            "fountain" => return LIGHTHOUSE_PALETTE,
            _ => {}
        }
    }
    if let Some(v) = tags.get("tourism") {
        if v == "artwork" {
            return STATUE_PALETTE;
        }
    }
    if let Some(v) = tags.get("building") {
        match v.as_str() {
            "industrial" | "warehouse" => return INDUSTRIAL_PALETTE,
            "house" | "detached" | "residential" | "apartments" | "terrace" => {
                return RESIDENTIAL_PALETTE
            }
            "church" | "cathedral" | "mosque" | "temple" | "synagogue" | "chapel" | "religious" => {
                return RELIGIOUS_PALETTE
            }
            "tower" | "clock_tower" => return TOWER_PALETTE,
            _ => {}
        }
    }
    DEFAULT_PALETTE
}

/// OSM `height=*` in meters; accepts bare numbers and trailing-m forms. Must be strictly positive.
fn parse_meters(raw: &str) -> Option<f64> {
    let s = raw.trim();
    let parsed = s.parse::<f64>().ok().or_else(|| {
        s.trim_end_matches(['m', 'M'])
            .trim_end()
            .parse::<f64>()
            .ok()
    })?;
    (parsed.is_finite() && parsed > 0.0).then_some(parsed)
}

fn is_glb(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == b"glTF"
}

/// GLB path: uniform intrinsic scale from OSM height, then voxelize.
fn voxelize_glb_for_placement(
    bytes: &[u8],
    placement: &Placement,
    args: &Args,
    ground_y: i32,
) -> Result<Vec<([i32; 3], Block)>, String> {
    let (bmin, bmax) = glb_model_bbox(bytes)?;
    let model_y = bmax[1] - bmin[1];
    let model_x = bmax[0] - bmin[0];
    let model_z = bmax[2] - bmin[2];

    let intrinsic_scale = match (placement.osm_height_m, placement.osm_xz_extent_m) {
        (Some(h), _) if model_y > 1e-4 => h as f32 / model_y,
        (_, Some(xz)) if model_x.max(model_z) > 1e-4 => xz as f32 / model_x.max(model_z),
        _ => 1.0,
    };
    if !intrinsic_scale.is_finite() || intrinsic_scale <= 0.0 {
        return Err("derived intrinsic_scale not finite/positive".into());
    }

    let final_y = model_y * intrinsic_scale;
    let final_x = model_x * intrinsic_scale;
    let final_z = model_z * intrinsic_scale;
    let max_xz = final_x.max(final_z);
    let max_overall = final_x.max(final_y).max(final_z);
    if max_xz > MAX_XZ_EXTENT_M || final_y > MAX_Y_EXTENT_M || max_overall < MIN_EXTENT_M {
        return Err(format!(
            "scaled extents {:.0}×{:.0}×{:.0} m outside caps",
            final_x, final_y, final_z
        ));
    }

    let transform = WorldTransform::new(
        0.0,
        intrinsic_scale as f64,
        [0.0, 0.0, 0.0],
        args.scale,
        placement.world_yaw_degrees,
        placement.anchor_x as f32,
        ground_y as f32,
        placement.anchor_z as f32,
    );
    voxelize_glb(bytes, transform)
}

fn voxelize_stl(
    bytes: &[u8],
    placement: &Placement,
    args: &Args,
    ground_y: i32,
) -> Result<Vec<([i32; 3], Block)>, String> {
    let triangles = parse_triangles(bytes)?;
    let (bmin, bmax) = bbox(&triangles).ok_or("no finite vertices")?;
    let fit = derive_fit(&bmin, &bmax, placement).ok_or("could not derive scale")?;

    let final_extents = {
        let raw = [bmax[0] - bmin[0], bmax[1] - bmin[1], bmax[2] - bmin[2]];
        [
            (raw[fit.axes[0].0] * fit.scale[0]).abs(),
            (raw[fit.axes[1].0] * fit.scale[1]).abs(),
            (raw[fit.axes[2].0] * fit.scale[2]).abs(),
        ]
    };
    let max_xz = final_extents[0].max(final_extents[2]);
    let max_overall = final_extents.iter().fold(0f32, |acc, e| acc.max(*e));
    if max_xz > MAX_XZ_EXTENT_M || final_extents[1] > MAX_Y_EXTENT_M || max_overall < MIN_EXTENT_M {
        return Err(format!(
            "scaled extents {:.0}×{:.0}×{:.0} m outside caps (XZ ≤ {:.0}, Y ≤ {:.0}, min {:.0})",
            final_extents[0],
            final_extents[1],
            final_extents[2],
            MAX_XZ_EXTENT_M,
            MAX_Y_EXTENT_M,
            MIN_EXTENT_M,
        ));
    }

    // Center the model's XZ bbox on the OSM anchor.
    let center_x = {
        let (src, sign) = fit.axes[0];
        (bmin[src] + bmax[src]) * 0.5 * sign * fit.scale[0]
    };
    let center_z = {
        let (src, sign) = fit.axes[2];
        (bmin[src] + bmax[src]) * 0.5 * sign * fit.scale[2]
    };

    let transform = WorldTransform::new(
        0.0,
        1.0,
        [0.0, 0.0, 0.0],
        args.scale,
        placement.world_yaw_degrees,
        placement.anchor_x as f32,
        ground_y as f32,
        placement.anchor_z as f32,
    );

    let fitted: Vec<[[f32; 3]; 3]> = triangles
        .iter()
        .map(|tri| {
            let centered = |v: [f32; 3]| {
                let p = apply_fit(v, &fit);
                [p[0] - center_x, p[1], p[2] - center_z]
            };
            [centered(tri[0]), centered(tri[1]), centered(tri[2])]
        })
        .collect();

    let mut voxels = voxelize_uniform_triangles(fitted, transform, STONE_BRICKS);
    let seed = qid_seed(&placement.qid);
    if let Some(layers) = &placement.palette_layers {
        let (y_min, y_max) = voxel_y_range(&voxels);
        for ([x, y, z], block) in voxels.iter_mut() {
            *block = pick_layered_block(layers, seed, [*x, *y, *z], y_min, y_max);
        }
    } else {
        for ([x, y, z], block) in voxels.iter_mut() {
            *block = pick_voxel_block(&placement.palette, seed, [*x, *y, *z]);
        }
    }
    Ok(voxels)
}

fn voxel_y_range(voxels: &[([i32; 3], Block)]) -> (i32, i32) {
    let mut min = i32::MAX;
    let mut max = i32::MIN;
    for (p, _) in voxels {
        if p[1] < min {
            min = p[1];
        }
        if p[1] > max {
            max = p[1];
        }
    }
    if min > max {
        (0, 1)
    } else {
        (min, max)
    }
}

/// Resolve JSON `palette_layers` → Block pools sorted by `y_max_frac` ascending.
fn resolve_palette_layers(
    layers: &[crate::models_3d::wikidata::index::PaletteLayer],
) -> Option<ResolvedLayers> {
    let mut entries: Vec<(f32, Vec<Block>)> = Vec::new();
    for l in layers {
        let blocks: Vec<Block> = if let Some(hex) = &l.hex {
            match color_text_to_rgb_tuple(&format!("#{}", hex.trim_start_matches('#'))) {
                Some(rgb) => closest_blocks(rgb, COLOR_PALETTE_K),
                None => continue,
            }
        } else {
            l.blocks
                .iter()
                .filter_map(|name| block_by_const_name(name))
                .collect()
        };
        if blocks.is_empty() {
            continue;
        }
        entries.push((l.y_max_frac, blocks));
    }
    if entries.is_empty() {
        return None;
    }
    entries.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (y_max_frac, blocks): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
    Some(ResolvedLayers { y_max_frac, blocks })
}

fn pick_layered_block(
    layers: &ResolvedLayers,
    seed: u64,
    pos: [i32; 3],
    y_min: i32,
    y_max: i32,
) -> Block {
    let span = (y_max - y_min).max(1) as f32;
    let frac = ((pos[1] - y_min) as f32 / span).clamp(0.0, 1.0);
    let idx = layers
        .y_max_frac
        .iter()
        .position(|&f| frac <= f)
        .unwrap_or(layers.y_max_frac.len() - 1);
    pick_voxel_block(&layers.blocks[idx], seed, pos)
}

/// Block-constant name → Block (case-insensitive); None on unknown name.
fn block_by_const_name(name: &str) -> Option<Block> {
    Some(match name.to_ascii_uppercase().as_str() {
        "ANDESITE" => ANDESITE,
        "BLACK_CONCRETE" => BLACK_CONCRETE,
        "BLACK_TERRACOTTA" => BLACK_TERRACOTTA,
        "BLACK_WOOL" => BLACK_WOOL,
        "BLACKSTONE" => BLACKSTONE,
        "BLUE_CONCRETE" => BLUE_CONCRETE,
        "BLUE_TERRACOTTA" => BLUE_TERRACOTTA,
        "BLUE_WOOL" => BLUE_WOOL,
        "BRICK" => BRICK,
        "BROWN_CONCRETE" => BROWN_CONCRETE,
        "BROWN_TERRACOTTA" => BROWN_TERRACOTTA,
        "CHISELED_STONE_BRICKS" => CHISELED_STONE_BRICKS,
        "COARSE_DIRT" => COARSE_DIRT,
        "COBBLED_DEEPSLATE" => COBBLED_DEEPSLATE,
        "COBBLESTONE" => COBBLESTONE,
        "CRACKED_STONE_BRICKS" => CRACKED_STONE_BRICKS,
        "CYAN_CONCRETE" => CYAN_CONCRETE,
        "CYAN_TERRACOTTA" => CYAN_TERRACOTTA,
        "DARK_OAK_LOG" => DARK_OAK_LOG,
        "DARK_OAK_PLANKS" => DARK_OAK_PLANKS,
        "DEEPSLATE" => DEEPSLATE,
        "DEEPSLATE_BRICKS" => DEEPSLATE_BRICKS,
        "DIORITE" => DIORITE,
        "DIRT" => DIRT,
        "END_STONE_BRICKS" => END_STONE_BRICKS,
        "GOLD_BLOCK" => GOLD_BLOCK,
        "GRANITE" => GRANITE,
        "GRAY_CONCRETE" => GRAY_CONCRETE,
        "GRAY_TERRACOTTA" => GRAY_TERRACOTTA,
        "GREEN_CONCRETE" => GREEN_CONCRETE,
        "GREEN_WOOL" => GREEN_WOOL,
        "HAY_BALE" => HAY_BALE,
        "IRON_BLOCK" => IRON_BLOCK,
        "LIGHT_BLUE_CONCRETE" => LIGHT_BLUE_CONCRETE,
        "LIGHT_BLUE_TERRACOTTA" => LIGHT_BLUE_TERRACOTTA,
        "LIGHT_GRAY_CONCRETE" => LIGHT_GRAY_CONCRETE,
        "LIGHT_GRAY_TERRACOTTA" => LIGHT_GRAY_TERRACOTTA,
        "LIME_CONCRETE" => LIME_CONCRETE,
        "MAGENTA_CONCRETE" => MAGENTA_CONCRETE,
        "MOSS_BLOCK" => MOSS_BLOCK,
        "MOSSY_COBBLESTONE" => MOSSY_COBBLESTONE,
        "MOSSY_STONE_BRICKS" => MOSSY_STONE_BRICKS,
        "MUD_BRICKS" => MUD_BRICKS,
        "NETHER_BRICK" => NETHER_BRICK,
        "NETHERITE_BLOCK" => NETHERITE_BLOCK,
        "OAK_LOG" => OAK_LOG,
        "OAK_PLANKS" => OAK_PLANKS,
        "ORANGE_TERRACOTTA" => ORANGE_TERRACOTTA,
        "ORANGE_WOOL" => ORANGE_WOOL,
        "POLISHED_ANDESITE" => POLISHED_ANDESITE,
        "POLISHED_BLACKSTONE" => POLISHED_BLACKSTONE,
        "POLISHED_BLACKSTONE_BRICKS" => POLISHED_BLACKSTONE_BRICKS,
        "POLISHED_DEEPSLATE" => POLISHED_DEEPSLATE,
        "POLISHED_DIORITE" => POLISHED_DIORITE,
        "POLISHED_GRANITE" => POLISHED_GRANITE,
        "PURPLE_CONCRETE" => PURPLE_CONCRETE,
        "QUARTZ_BLOCK" => QUARTZ_BLOCK,
        "QUARTZ_BRICKS" => QUARTZ_BRICKS,
        "RED_CONCRETE" => RED_CONCRETE,
        "RED_NETHER_BRICKS" => RED_NETHER_BRICKS,
        "RED_TERRACOTTA" => RED_TERRACOTTA,
        "RED_WOOL" => RED_WOOL,
        "SANDSTONE" => SANDSTONE,
        "SMOOTH_QUARTZ" => SMOOTH_QUARTZ,
        "SMOOTH_SANDSTONE" => SMOOTH_SANDSTONE,
        "SMOOTH_STONE" => SMOOTH_STONE,
        "SNOW_BLOCK" => SNOW_BLOCK,
        "SPRUCE_LOG" => SPRUCE_LOG,
        "SPRUCE_PLANKS" => SPRUCE_PLANKS,
        "STONE" => STONE,
        "STONE_BRICKS" => STONE_BRICKS,
        "TERRACOTTA" => TERRACOTTA,
        "WAXED_COPPER_BLOCK" => WAXED_COPPER_BLOCK,
        "WAXED_EXPOSED_COPPER" => WAXED_EXPOSED_COPPER,
        "WAXED_OXIDIZED_COPPER" => WAXED_OXIDIZED_COPPER,
        "WHITE_CONCRETE" => WHITE_CONCRETE,
        "WHITE_TERRACOTTA" => WHITE_TERRACOTTA,
        "WHITE_WOOL" => WHITE_WOOL,
        "YELLOW_CONCRETE" => YELLOW_CONCRETE,
        "YELLOW_TERRACOTTA" => YELLOW_TERRACOTTA,
        "YELLOW_WOOL" => YELLOW_WOOL,
        _ => return None,
    })
}

/// Deterministic per-voxel palette pick via splitmix64 (avoids pattern artifacts).
fn pick_voxel_block(palette: &[Block], seed: u64, pos: [i32; 3]) -> Block {
    if palette.is_empty() {
        return STONE_BRICKS;
    }
    let mut h = seed;
    for c in pos {
        h ^= c as i64 as u64;
        h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
        h ^= h >> 31;
    }
    palette[(h as usize) % palette.len()]
}

fn qid_seed(qid: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in qid.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Axis permutation + per-axis scale to fit an STL into OSM's expected dimensions.
#[derive(Clone, Copy, Debug)]
struct ModelFit {
    axes: [(usize, f32); 3],
    scale: [f32; 3],
}

/// Up-axis from model extents. Default Z (STL convention); override when a clear outlier exists.
fn pick_up_axis(extents: &[f32; 3]) -> usize {
    let mut sorted: [(usize, f32); 3] = [(0, extents[0]), (1, extents[1]), (2, extents[2])];
    sorted.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (min_idx, min_val) = sorted[0];
    let (_, med_val) = sorted[1];
    let (max_idx, max_val) = sorted[2];

    if med_val <= 1e-3 {
        return 2;
    }
    if max_val / med_val > 1.10 {
        return max_idx;
    }
    if min_val / med_val < 0.5 {
        return min_idx;
    }
    2
}

/// Orientation + per-axis scale to fit the STL inside OSM's footprint and height.
fn derive_fit(bmin: &[f32; 3], bmax: &[f32; 3], p: &Placement) -> Option<ModelFit> {
    let extents = [bmax[0] - bmin[0], bmax[1] - bmin[1], bmax[2] - bmin[2]];
    if extents.iter().all(|&e| e < 1e-3) {
        return None;
    }

    let up = pick_up_axis(&extents);

    let axes: [(usize, f32); 3] = match up {
        0 => [(1, -1.0), (0, 1.0), (2, 1.0)],
        1 => [(0, 1.0), (1, 1.0), (2, 1.0)],
        2 => [(0, 1.0), (2, 1.0), (1, -1.0)],
        _ => unreachable!(),
    };

    let world_extents = [extents[axes[0].0], extents[axes[1].0], extents[axes[2].0]];

    let scale_y = match p.osm_height_m {
        Some(h) if world_extents[1] > 1e-3 => Some(h as f32 / world_extents[1]),
        _ => None,
    };
    let scale_xz = match p.osm_xz_extent_m {
        Some(xz) => {
            let max_xz = world_extents[0].max(world_extents[2]);
            if max_xz > 1e-3 {
                Some(xz as f32 / max_xz)
            } else {
                None
            }
        }
        None => None,
    };

    let (sx, sy, sz) = match (scale_xz, scale_y) {
        (Some(xz), Some(y)) => (xz, y, xz),
        (Some(xz), None) => (xz, xz, xz),
        (None, Some(y)) => (y, y, y),
        (None, None) => return None,
    };
    if [sx, sy, sz].iter().any(|s| !s.is_finite() || *s <= 0.0) {
        return None;
    }
    Some(ModelFit {
        axes,
        scale: [sx, sy, sz],
    })
}

#[inline]
fn apply_fit(v: [f32; 3], fit: &ModelFit) -> [f32; 3] {
    let mut out = [0f32; 3];
    for (axis, slot) in out.iter_mut().enumerate() {
        let (src, sign) = fit.axes[axis];
        *slot = v[src] * sign * fit.scale[axis];
    }
    out
}

fn anchor_xz(element: &ProcessedElement) -> Option<(i32, i32)> {
    match element {
        ProcessedElement::Node(n) => Some((n.x, n.z)),
        ProcessedElement::Way(w) => centroid(w.nodes.iter().map(|n| (n.x, n.z))),
        ProcessedElement::Relation(r) => centroid(
            r.members
                .iter()
                .flat_map(|m| m.way.nodes.iter().map(|n| (n.x, n.z))),
        ),
    }
}

fn centroid<I: Iterator<Item = (i32, i32)>>(coords: I) -> Option<(i32, i32)> {
    let mut sx: i64 = 0;
    let mut sz: i64 = 0;
    let mut count: i64 = 0;
    for (x, z) in coords {
        sx += x as i64;
        sz += z as i64;
        count += 1;
    }
    if count == 0 {
        None
    } else {
        Some(((sx / count) as i32, (sz / count) as i32))
    }
}

fn osm_bbox(element: &ProcessedElement) -> Option<Bbox> {
    let mut iter: Box<dyn Iterator<Item = (i32, i32)>> = match element {
        ProcessedElement::Node(n) => Box::new(std::iter::once((n.x, n.z))),
        ProcessedElement::Way(w) => Box::new(w.nodes.iter().map(|n| (n.x, n.z))),
        ProcessedElement::Relation(r) => Box::new(
            r.members
                .iter()
                .flat_map(|m| m.way.nodes.iter().map(|n| (n.x, n.z))),
        ),
    };
    let (x0, z0) = iter.next()?;
    let mut min_x = x0;
    let mut max_x = x0;
    let mut min_z = z0;
    let mut max_z = z0;
    for (x, z) in iter {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    Some(Bbox {
        min_x,
        min_z,
        max_x,
        max_z,
    })
}

fn parse_direction(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if let Ok(deg) = s.parse::<f64>() {
        if deg.is_finite() {
            return Some(deg.rem_euclid(360.0));
        }
    }
    let deg = match s.to_ascii_uppercase().as_str() {
        "N" | "NORTH" => 0.0,
        "NNE" => 22.5,
        "NE" => 45.0,
        "ENE" => 67.5,
        "E" | "EAST" => 90.0,
        "ESE" => 112.5,
        "SE" => 135.0,
        "SSE" => 157.5,
        "S" | "SOUTH" => 180.0,
        "SSW" => 202.5,
        "SW" => 225.0,
        "WSW" => 247.5,
        "W" | "WEST" => 270.0,
        "WNW" => 292.5,
        "NW" => 315.0,
        "NNW" => 337.5,
        _ => return None,
    };
    Some(deg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_to_rgb_known() {
        assert_eq!(material_to_rgb("brick"), Some((151, 98, 83)));
        assert_eq!(material_to_rgb("Stone"), Some((132, 135, 134)));
        assert_eq!(material_to_rgb("nonsense"), None);
    }

    fn block_ids(palette: &[Block]) -> Vec<u16> {
        palette.iter().map(|b| b.id()).collect()
    }

    #[test]
    fn structure_palette_dispatches_to_category() {
        use std::collections::HashMap as M;
        let mut t = M::new();
        t.insert("man_made".to_string(), "tower".to_string());
        assert_eq!(
            block_ids(structure_type_palette(&t)),
            block_ids(TOWER_PALETTE)
        );

        t.clear();
        t.insert("historic".to_string(), "castle".to_string());
        assert_eq!(
            block_ids(structure_type_palette(&t)),
            block_ids(HISTORIC_PALETTE)
        );

        t.clear();
        t.insert("building".to_string(), "industrial".to_string());
        assert_eq!(
            block_ids(structure_type_palette(&t)),
            block_ids(INDUSTRIAL_PALETTE)
        );

        t.clear();
        t.insert("amenity".to_string(), "place_of_worship".to_string());
        assert_eq!(
            block_ids(structure_type_palette(&t)),
            block_ids(RELIGIOUS_PALETTE)
        );

        t.clear();
        assert_eq!(
            block_ids(structure_type_palette(&t)),
            block_ids(DEFAULT_PALETTE)
        );
    }

    #[test]
    fn pick_voxel_block_deterministic_and_well_distributed() {
        let palette = TOWER_PALETTE;
        let seed = qid_seed("Q41225");
        // Same input → same block.
        let b0 = pick_voxel_block(palette, seed, [0, 0, 0]);
        let b0_again = pick_voxel_block(palette, seed, [0, 0, 0]);
        assert_eq!(b0.id(), b0_again.id());

        // Sample 10×10×10; every palette block should appear and none should exceed 40%.
        let mut counts = std::collections::HashMap::new();
        for x in 0..10 {
            for y in 0..10 {
                for z in 0..10 {
                    let b = pick_voxel_block(palette, seed, [x, y, z]);
                    *counts.entry(b.id()).or_insert(0usize) += 1;
                }
            }
        }
        assert_eq!(
            counts.len(),
            palette.len(),
            "every palette block should appear: {counts:?}"
        );
        let total: usize = counts.values().sum();
        let max = *counts.values().max().unwrap();
        assert!(
            (max as f32 / total as f32) < 0.40,
            "block distribution skewed (max {max}/{total}): {counts:?}"
        );
    }

    fn placement(h: Option<f64>, xz: Option<f64>) -> Placement {
        Placement {
            osm_id: 1,
            qid: "Q1".into(),
            anchor_x: 0,
            anchor_z: 0,
            footprint: Bbox {
                min_x: 0,
                max_x: 1,
                min_z: 0,
                max_z: 1,
            },
            world_yaw_degrees: 0.0,
            palette: vec![STONE_BRICKS],
            palette_layers: None,
            osm_height_m: h,
            osm_xz_extent_m: xz,
        }
    }

    #[test]
    fn derive_fit_yup_tower_renders_at_osm_dimensions() {
        // Y-up STL: 10 wide × 100 tall × 10 deep. OSM says 96m tall × 12m footprint.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[10.0, 100.0, 10.0],
            &placement(Some(96.0), Some(12.0)),
        )
        .unwrap();
        assert_eq!(fit.axes[1].0, 1, "Y stays Y for Y-up tower");
        // Per-axis scale: Y to fit 96m on 100, XZ to fit 12m on 10.
        assert!((fit.scale[1] - 0.96).abs() < 1e-3);
        assert!((fit.scale[0] - 1.2).abs() < 1e-3);
        assert!((fit.scale[2] - 1.2).abs() < 1e-3);
    }

    #[test]
    fn derive_fit_zup_tower_swaps_axes() {
        // Z-up STL: 10 × 10 × 100 (tower along Z). OSM says 96m tall × 12m footprint.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[10.0, 10.0, 100.0],
            &placement(Some(96.0), Some(12.0)),
        )
        .unwrap();
        // Up axis is the longest: index 2 (Z). Z gets remapped onto world Y.
        assert_eq!(fit.axes[1].0, 2, "Z became world Y");
        // World-Y extent in model space is 100, scale should bring it to 96m.
        assert!((fit.scale[1] - 0.96).abs() < 1e-3);
        // World-X/Z extents come from model X/Y (both 10), scale to 12m.
        assert!((fit.scale[0] - 1.2).abs() < 1e-3);
        assert!((fit.scale[2] - 1.2).abs() < 1e-3);
    }

    #[test]
    fn derive_fit_wide_zup_temple_uses_shortest_as_up() {
        // Z-up temple STL 50×50×10; min-as-up override should pick Z.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[50.0, 50.0, 10.0],
            &placement(Some(10.0), Some(30.0)),
        )
        .unwrap();
        // Z (index 2) is the shortest extent → becomes world Y.
        assert_eq!(fit.axes[1].0, 2, "Z (shortest) became world Y");
        assert!((fit.scale[1] - 1.0).abs() < 1e-3);
        assert!((fit.scale[0] - 0.6).abs() < 1e-3);
    }

    #[test]
    fn derive_fit_cubic_defaults_to_zup() {
        // Cubic STL (20×20×20). STL convention is Z-up, default to Z.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[20.0, 20.0, 20.0],
            &placement(Some(10.0), Some(10.0)),
        )
        .unwrap();
        assert_eq!(fit.axes[1].0, 2);
    }

    #[test]
    fn derive_fit_arc_de_triomphe_real_extents() {
        // Real Arc de Triomphe extents — falls through to Z-up default.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[24.5, 15.24, 22.6],
            &placement(Some(49.54), Some(45.0)),
        )
        .unwrap();
        assert_eq!(fit.axes[1].0, 2, "STL is Z-up; default fires");
    }

    #[test]
    fn derive_fit_clearly_elongated_picks_longest() {
        // 10×100×10 — clearly elongated along Y. max/median = 10 → override fires.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[10.0, 100.0, 10.0],
            &placement(Some(96.0), Some(12.0)),
        )
        .unwrap();
        assert_eq!(
            fit.axes[1].0, 1,
            "longest axis (Y) wins via elongated override"
        );
    }

    #[test]
    fn derive_fit_returns_none_when_no_constraints() {
        let p = placement(None, None);
        assert!(derive_fit(&[0.0, 0.0, 0.0], &[10.0, 10.0, 10.0], &p).is_none());
    }

    #[test]
    fn apply_fit_zup_to_yup_preserves_handedness() {
        let fit = ModelFit {
            axes: [(0, 1.0), (2, 1.0), (1, -1.0)],
            scale: [1.0, 1.0, 1.0],
        };
        // (1, 0, 0) stays (1, 0, 0)
        assert_eq!(apply_fit([1.0, 0.0, 0.0], &fit), [1.0, 0.0, 0.0]);
        // STL +Z (height) becomes world +Y
        assert_eq!(apply_fit([0.0, 0.0, 5.0], &fit), [0.0, 5.0, 0.0]);
        // STL +Y becomes world -Z (handedness preserved)
        assert_eq!(apply_fit([0.0, 3.0, 0.0], &fit), [0.0, 0.0, -3.0]);
    }
}
