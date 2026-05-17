//! Wikidata pre-scan + scale/color/orient resolution + placement.

use crate::args::Args;
use crate::block_definitions::*;
use crate::colors::{color_text_to_rgb_tuple, RGBTuple};
use crate::models_3d::palette::closest_block;
use crate::models_3d::voxelize::{voxelize_uniform_triangles, WorldTransform};
use crate::models_3d::wikidata::client::fetch_stl;
use crate::models_3d::wikidata::index::lookup;
use crate::models_3d::wikidata::stl::{bbox, parse_triangles};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

/// Reject Wikidata models whose final bbox diagonal exceeds this — guards
/// against "Penang island.stl"-class entries and scale-heuristic misfires.
const MAX_BBOX_DIAGONAL_M: f32 = 500.0;
const MIN_BBOX_DIAGONAL_M: f32 = 2.0;

#[derive(Clone, Debug)]
struct Placement {
    osm_id: u64,
    qid: String,
    anchor_x: i32,
    anchor_z: i32,
    footprint: Bbox,
    world_yaw_degrees: f64,
    block: Block,
    /// OSM-resolved target height in meters; `None` falls back to bbox-XZ scaling.
    osm_height_m: Option<f64>,
    /// OSM-resolved target XZ extent in meters; `None` falls back to height-only scaling.
    osm_xz_extent_m: Option<f64>,
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
    pub suppressed_ids: HashSet<u64>,
    placements: Vec<Placement>,
}

impl PrescanResult {
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }
}

/// Scans for `wikidata=Q*` tags whose QID matches the bundled index. Suppresses
/// the tagged element and any building-like element inside its footprint.
/// Elements already suppressed by the 3DMR pre-scan are skipped — 3DMR wins.
pub fn prescan(
    elements: &[ProcessedElement],
    already_suppressed: &HashSet<u64>,
    world_rotation: f64,
    args_scale: f64,
) -> PrescanResult {
    let mut placements = Vec::new();
    let mut suppressed = HashSet::new();
    let mut footprints: Vec<Bbox> = Vec::new();

    for element in elements {
        if already_suppressed.contains(&element.id()) {
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

        let raw_footprint = osm_bbox(element);
        let footprint = raw_footprint.unwrap_or_else(|| Bbox {
            min_x: anchor_x - 8,
            max_x: anchor_x + 8,
            min_z: anchor_z - 8,
            max_z: anchor_z + 8,
        });

        let osm_height_m = element
            .tags()
            .get("height")
            .and_then(|s| s.trim().parse::<f64>().ok())
            .or(entry.height_m);

        let osm_xz_extent_m = raw_footprint.map(|b| {
            let dx = (b.max_x - b.min_x) as f64;
            let dz = (b.max_z - b.min_z) as f64;
            dx.max(dz) / args_scale
        });

        let osm_yaw = element
            .tags()
            .get("direction")
            .and_then(|s| parse_direction(s))
            .unwrap_or(0.0);
        let world_yaw_degrees = osm_yaw + world_rotation;

        let block = resolve_block(element);

        suppressed.insert(element.id());
        if let Some(b) = raw_footprint {
            footprints.push(b);
        }
        placements.push(Placement {
            osm_id: element.id(),
            qid: qid.to_string(),
            anchor_x,
            anchor_z,
            footprint,
            world_yaw_degrees,
            block,
            osm_height_m,
            osm_xz_extent_m,
        });
    }

    if !footprints.is_empty() {
        for element in elements {
            if suppressed.contains(&element.id()) || already_suppressed.contains(&element.id()) {
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
                suppressed.insert(element.id());
            }
        }
    }

    PrescanResult {
        suppressed_ids: suppressed,
        placements,
    }
}

/// Fetches STLs (with cache), voxelizes them, places blocks.
pub fn place_wikidata_models(editor: &mut WorldEditor, args: &Args, prescan: &PrescanResult) {
    if prescan.placements.is_empty() {
        return;
    }

    println!(
        "{} Fetching {} Wikidata 3D model{}...",
        "  [+]".bold(),
        prescan.placements.len(),
        if prescan.placements.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    let unique_urls: Vec<String> = {
        let mut set: HashSet<String> = HashSet::new();
        for p in &prescan.placements {
            if let Some(e) = lookup(&p.qid) {
                set.insert(e.url.clone());
            }
        }
        set.into_iter().collect()
    };

    let fetched: HashMap<String, Vec<u8>> = unique_urls
        .par_iter()
        .filter_map(|url| match fetch_stl(url) {
            Ok(bytes) => Some((url.clone(), bytes)),
            Err(e) => {
                eprintln!(
                    "{} Wikidata STL fetch failed ({url}): {e}",
                    "Warning:".yellow().bold()
                );
                None
            }
        })
        .collect();

    let mut placed = 0usize;
    let mut total_voxels = 0usize;

    for placement in &prescan.placements {
        let Some(entry) = lookup(&placement.qid) else {
            continue;
        };
        let Some(stl_bytes) = fetched.get(&entry.url) else {
            continue;
        };

        let triangles = match parse_triangles(stl_bytes) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "{} Wikidata STL parse failed ({}, OSM {}): {e}",
                    "Warning:".yellow().bold(),
                    placement.qid,
                    placement.osm_id
                );
                continue;
            }
        };

        let Some((bmin, bmax)) = bbox(&triangles) else {
            eprintln!(
                "{} Wikidata model {} has no finite vertices, skipping",
                "Warning:".yellow().bold(),
                placement.qid
            );
            continue;
        };

        let Some(fit) = derive_fit(&bmin, &bmax, placement) else {
            eprintln!(
                "{} Wikidata model {} (OSM {}): could not derive scale, skipping",
                "Warning:".yellow().bold(),
                placement.qid,
                placement.osm_id
            );
            continue;
        };

        // After-fit world-space extents — sanity-check on the OSM-implied size
        // rather than the raw STL, so towers (small XZ × large Y) aren't rejected.
        let final_extents = {
            let raw = [bmax[0] - bmin[0], bmax[1] - bmin[1], bmax[2] - bmin[2]];
            [
                raw[fit.axes[0].0] * fit.scale[0],
                raw[fit.axes[1].0] * fit.scale[1],
                raw[fit.axes[2].0] * fit.scale[2],
            ]
        };
        let max_extent = final_extents.iter().fold(0f32, |acc, e| acc.max(e.abs()));
        if !(MIN_BBOX_DIAGONAL_M..=MAX_BBOX_DIAGONAL_M).contains(&max_extent) {
            eprintln!(
                "{} Wikidata model {} (OSM {}): scaled max extent {:.1}m outside [{:.0}, {:.0}], skipping",
                "Warning:".yellow().bold(),
                placement.qid,
                placement.osm_id,
                max_extent,
                MIN_BBOX_DIAGONAL_M,
                MAX_BBOX_DIAGONAL_M
            );
            continue;
        }

        let ground_y = lowest_ground_in_bbox(editor, &placement.footprint);

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
                [
                    apply_fit(tri[0], &fit),
                    apply_fit(tri[1], &fit),
                    apply_fit(tri[2], &fit),
                ]
            })
            .collect();

        let mut voxels = voxelize_uniform_triangles(fitted, transform, placement.block);

        if let Some(min_voxel_y) = voxels.iter().map(|(p, _)| p[1]).min() {
            let dy = ground_y - min_voxel_y;
            if dy != 0 {
                for (pos, _) in voxels.iter_mut() {
                    pos[1] += dy;
                }
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

/// Returns a single Block representing the OSM element's dominant material/color.
/// Priority: `building:colour` / `colour` / hex → `building:material` mapping →
/// structure-type heuristic → STONE_BRICKS.
fn resolve_block(element: &ProcessedElement) -> Block {
    let tags = element.tags();
    if let Some(c) = tags
        .get("building:colour")
        .or_else(|| tags.get("colour"))
        .or_else(|| tags.get("color"))
    {
        if let Some(rgb) = color_text_to_rgb_tuple(c) {
            return closest_block(rgb);
        }
    }
    if let Some(m) = tags.get("building:material") {
        if let Some(rgb) = material_to_rgb(m) {
            return closest_block(rgb);
        }
    }
    structure_type_default(tags)
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

fn structure_type_default(tags: &std::collections::HashMap<String, String>) -> Block {
    if let Some(v) = tags.get("man_made") {
        match v.as_str() {
            "tower" | "obelisk" | "chimney" => return STONE,
            "lighthouse" => return WHITE_CONCRETE,
            "monument" => return SMOOTH_STONE,
            _ => {}
        }
    }
    if let Some(v) = tags.get("historic") {
        match v.as_str() {
            "castle" | "fort" | "ruins" | "city_gate" => return COBBLESTONE,
            "memorial" | "monument" => return SMOOTH_STONE,
            "archaeological_site" => return MOSSY_COBBLESTONE,
            _ => {}
        }
    }
    if let Some(v) = tags.get("amenity") {
        match v.as_str() {
            "place_of_worship" => return SANDSTONE,
            "fountain" => return WHITE_CONCRETE,
            _ => {}
        }
    }
    if let Some(v) = tags.get("tourism") {
        if v == "artwork" {
            return SMOOTH_STONE;
        }
    }
    if let Some(v) = tags.get("building") {
        match v.as_str() {
            "industrial" | "warehouse" => return GRAY_CONCRETE,
            "house" | "detached" | "residential" | "apartments" | "terrace" => return BRICK,
            "church" | "cathedral" | "mosque" | "temple" | "synagogue" | "chapel" | "religious" => {
                return SANDSTONE
            }
            _ => {}
        }
    }
    STONE_BRICKS
}

/// Resolved fit between an STL's raw geometry and OSM's expected dimensions.
/// `axes` permutes (and optionally negates) model axes to world axes — world Y
/// becomes the up axis of the rendered structure. `scale` is per-world-axis
/// in meters per model-unit, applied after the axis remap.
///
/// Example: for a Z-up STL of a 200-unit-tall tower whose OSM element says
/// height=96m, footprint=12m, this produces axes that swap Z→Y and scales
/// such that the rendered model is 12m wide × 96m tall × 12m deep.
#[derive(Clone, Copy, Debug)]
struct ModelFit {
    axes: [(usize, f32); 3],
    scale: [f32; 3],
}

/// Picks which model-space axis corresponds to "up" by combining the model's
/// own geometry with the OSM tagging. Tall OSM landmarks (height ≫ footprint)
/// align with the model's longest extent; otherwise default to Y-up.
fn pick_up_axis(extents: &[f32; 3], osm_height: Option<f64>, osm_xz: Option<f64>) -> usize {
    let osm_aspect = match (osm_height, osm_xz) {
        (Some(h), Some(xz)) if xz > 1e-6 => h / xz,
        _ => 1.0,
    };
    if osm_aspect > 1.5 {
        let mut max_idx = 0usize;
        let mut max_val = extents[0];
        for (i, &e) in extents.iter().enumerate().skip(1) {
            if e > max_val {
                max_val = e;
                max_idx = i;
            }
        }
        max_idx
    } else {
        1
    }
}

/// Derives the full model fit (orientation + per-axis scale) that places the
/// STL inside OSM's expected footprint and height. Non-uniform scaling lets
/// thin towers and wide buildings both end up the right size even when the
/// raw STL aspect ratio doesn't match reality.
fn derive_fit(bmin: &[f32; 3], bmax: &[f32; 3], p: &Placement) -> Option<ModelFit> {
    let extents = [bmax[0] - bmin[0], bmax[1] - bmin[1], bmax[2] - bmin[2]];
    if extents.iter().all(|&e| e < 1e-3) {
        return None;
    }

    let up = pick_up_axis(&extents, p.osm_height_m, p.osm_xz_extent_m);

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

fn lowest_ground_in_bbox(editor: &WorldEditor, bbox: &Bbox) -> i32 {
    let dx = bbox.max_x - bbox.min_x;
    let dz = bbox.max_z - bbox.min_z;
    let stride = (dx.max(dz) / 16).clamp(1, 8);
    let mut lowest = i32::MAX;
    let mut x = bbox.min_x;
    while x <= bbox.max_x {
        let mut z = bbox.min_z;
        while z <= bbox.max_z {
            lowest = lowest.min(editor.get_ground_level(x, z));
            z += stride;
        }
        x += stride;
    }
    for (x, z) in [
        (bbox.min_x, bbox.min_z),
        (bbox.max_x, bbox.min_z),
        (bbox.min_x, bbox.max_z),
        (bbox.max_x, bbox.max_z),
    ] {
        lowest = lowest.min(editor.get_ground_level(x, z));
    }
    if lowest == i32::MAX {
        editor.get_ground_level((bbox.min_x + bbox.max_x) / 2, (bbox.min_z + bbox.max_z) / 2)
    } else {
        lowest
    }
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

    #[test]
    fn structure_type_defaults() {
        use std::collections::HashMap as M;
        let mut t = M::new();
        t.insert("man_made".to_string(), "tower".to_string());
        assert_eq!(structure_type_default(&t).id(), STONE.id());

        t.clear();
        t.insert("historic".to_string(), "castle".to_string());
        assert_eq!(structure_type_default(&t).id(), COBBLESTONE.id());

        t.clear();
        t.insert("building".to_string(), "industrial".to_string());
        assert_eq!(structure_type_default(&t).id(), GRAY_CONCRETE.id());

        t.clear();
        assert_eq!(structure_type_default(&t).id(), STONE_BRICKS.id());
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
            block: STONE_BRICKS,
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
    fn derive_fit_wide_building_keeps_yup() {
        // Stocky building: 50 × 20 × 50 in STL. OSM 10m tall × 30m wide → no clear tall axis.
        let fit = derive_fit(
            &[0.0, 0.0, 0.0],
            &[50.0, 20.0, 50.0],
            &placement(Some(10.0), Some(30.0)),
        )
        .unwrap();
        assert_eq!(fit.axes[1].0, 1, "no aspect-driven swap for wide buildings");
        assert!((fit.scale[1] - 0.5).abs() < 1e-3);
        assert!((fit.scale[0] - 0.6).abs() < 1e-3);
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
