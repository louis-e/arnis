//! Stadium archetype with footprint-fit GLB voxelization.

use crate::args::Args;
use crate::models_3d::custom::client;
use crate::models_3d::voxelize::{glb_model_bbox, voxelize_glb, WorldTransform};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use std::collections::HashSet;

const MODEL_URL: &str = "https://arnismc.com/assets/3dmodels/stadium.glb";
const CACHE_FILE: &str = "stadium.glb";

const MIN_SHORT_EXTENT_M: f32 = 10.0;
/// Caps to avoid voxelizing absurd polygons (entire sports complexes mis-tagged as one stadium).
const MAX_LONG_EXTENT_M: f32 = 500.0;
const MAX_SHORT_EXTENT_M: f32 = 400.0;
const MAX_HEIGHT_M: f32 = 200.0;
/// `leisure=stadium` qualifies on its own above this footprint area (m²).
const LARGE_STADIUM_AREA_M2: f64 = 20_000.0;
/// Smaller `leisure=stadium` needs an inner `building=stadium` to qualify.
const MEDIUM_STADIUM_AREA_M2: f64 = 10_000.0;
const DEFAULT_HEIGHT_M: f32 = 28.0;
const HEIGHT_MULTIPLIER: f32 = 1.5;

#[derive(Clone, Debug)]
struct Placement {
    osm_id: u64,
    anchor_x: i32,
    anchor_z: i32,
    footprint: Bbox,
    long_m: f32,
    short_m: f32,
    /// Long-axis bearing in degrees, CCW from world +X.
    yaw_degrees: f64,
    osm_height_m: Option<f32>,
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
    model_bytes: Option<Vec<u8>>,
}

impl PrescanResult {
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Regions each placement may write to (stream-to-disk deferral). The model is a
    /// rotated long×short rectangle centred on the anchor, so its half-diagonal (+ ring) bounds it.
    pub fn deferred_region_keys(&self, scale: f64) -> Vec<(i32, i32)> {
        self.placements
            .iter()
            .flat_map(|p| {
                let r = (p.long_m.hypot(p.short_m) as f64 * 0.5 * scale).ceil() as i32;
                crate::models_3d::region_keys_around(p.anchor_x, p.anchor_z, r)
            })
            .collect()
    }
}

pub fn prescan(
    elements: &[ProcessedElement],
    already_suppressed: &HashSet<(&'static str, u64)>,
    args_scale: f64,
) -> PrescanResult {
    let (placements, mut suppressed, footprints) =
        collect_stadium_placements(elements, already_suppressed, args_scale);

    if placements.is_empty() {
        return PrescanResult {
            suppressed_ids: suppressed,
            placements,
            model_bytes: None,
        };
    }

    // On fetch failure, drop suppression so inner features still render procedurally.
    let model_bytes = match client::fetch_glb(MODEL_URL, CACHE_FILE) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "{} stadium model fetch failed ({MODEL_URL}): {e}",
                "Warning:".yellow().bold()
            );
            return PrescanResult {
                suppressed_ids: HashSet::new(),
                placements: Vec::new(),
                model_bytes: None,
            };
        }
    };

    let interior =
        collect_interior_suppression(elements, already_suppressed, &suppressed, &footprints);
    suppressed.extend(interior);

    PrescanResult {
        suppressed_ids: suppressed,
        placements,
        model_bytes: Some(model_bytes),
    }
}

fn collect_stadium_placements(
    elements: &[ProcessedElement],
    already_suppressed: &HashSet<(&'static str, u64)>,
    args_scale: f64,
) -> (Vec<Placement>, HashSet<(&'static str, u64)>, Vec<Bbox>) {
    let building_stadium_anchors: Vec<(i32, i32)> = elements
        .iter()
        .filter(|e| e.tags().get("building").map(|s| s.as_str()) == Some("stadium"))
        .filter_map(anchor_xz)
        .collect();

    let mut placements: Vec<Placement> = Vec::new();
    let mut suppressed: HashSet<(&'static str, u64)> = HashSet::new();
    let mut footprints: Vec<Bbox> = Vec::new();

    for element in elements {
        let key = (element.kind(), element.id());
        if already_suppressed.contains(&key) {
            continue;
        }
        if element.tags().get("leisure").map(|s| s.as_str()) != Some("stadium") {
            continue;
        }
        let Some(p) = build_placement(element, args_scale) else {
            continue;
        };
        if !leisure_stadium_qualifies(&p, &building_stadium_anchors) {
            continue;
        }
        suppressed.insert(key);
        footprints.push(p.footprint);
        placements.push(p);
    }

    for element in elements {
        let key = (element.kind(), element.id());
        if already_suppressed.contains(&key) || suppressed.contains(&key) {
            continue;
        }
        if element.tags().get("building").map(|s| s.as_str()) != Some("stadium") {
            continue;
        }
        let Some((cx, cz)) = anchor_xz(element) else {
            continue;
        };
        if footprints.iter().any(|b| b.contains(cx, cz)) {
            suppressed.insert(key);
            continue;
        }
        let Some(p) = build_placement(element, args_scale) else {
            continue;
        };
        if footprint_area_m2(&p) < LARGE_STADIUM_AREA_M2 {
            continue;
        }
        suppressed.insert(key);
        footprints.push(p.footprint);
        placements.push(p);
    }

    (placements, suppressed, footprints)
}

fn footprint_area_m2(p: &Placement) -> f64 {
    p.long_m as f64 * p.short_m as f64
}

fn leisure_stadium_qualifies(p: &Placement, building_anchors: &[(i32, i32)]) -> bool {
    let area = footprint_area_m2(p);
    if area >= LARGE_STADIUM_AREA_M2 {
        return true;
    }
    if area < MEDIUM_STADIUM_AREA_M2 {
        return false;
    }
    building_anchors
        .iter()
        .any(|&(x, z)| p.footprint.contains(x, z))
}

fn collect_interior_suppression(
    elements: &[ProcessedElement],
    already_suppressed: &HashSet<(&'static str, u64)>,
    claimed: &HashSet<(&'static str, u64)>,
    footprints: &[Bbox],
) -> HashSet<(&'static str, u64)> {
    let mut interior: HashSet<(&'static str, u64)> = HashSet::new();
    if footprints.is_empty() {
        return interior;
    }
    for element in elements {
        let key = (element.kind(), element.id());
        if already_suppressed.contains(&key) || claimed.contains(&key) {
            continue;
        }
        if !is_suppressible(element) {
            continue;
        }
        let Some((cx, cz)) = anchor_xz(element) else {
            continue;
        };
        if footprints.iter().any(|b| b.contains(cx, cz)) {
            interior.insert(key);
        }
    }
    interior
}

fn is_suppressible(element: &ProcessedElement) -> bool {
    let tags = element.tags();
    if tags.contains_key("building") || tags.contains_key("building:part") {
        return true;
    }
    matches!(
        tags.get("leisure").map(|s| s.as_str()),
        Some("pitch") | Some("track")
    )
}

fn build_placement(element: &ProcessedElement, args_scale: f64) -> Option<Placement> {
    let points = polygon_points(element)?;
    if points.len() < 3 {
        return None;
    }
    let footprint = bbox_of(&points)?;

    let (long_blocks, short_blocks, theta) = principal_axis(&points)?;
    let long_m = (long_blocks / args_scale) as f32;
    let short_m = (short_blocks / args_scale) as f32;
    if short_m < MIN_SHORT_EXTENT_M || long_m > MAX_LONG_EXTENT_M || short_m > MAX_SHORT_EXTENT_M {
        return None;
    }

    let (cx, cz) = centroid(&points)?;
    let osm_height_m = element
        .tags()
        .get("height")
        .and_then(|s| parse_meters(s))
        .map(|m| (m as f32).min(MAX_HEIGHT_M));

    Some(Placement {
        osm_id: element.id(),
        anchor_x: cx,
        anchor_z: cz,
        footprint,
        long_m,
        short_m,
        yaw_degrees: theta.to_degrees(),
        osm_height_m,
    })
}

pub fn place_stadium_models(editor: &mut WorldEditor, args: &Args, prescan: &PrescanResult) {
    if prescan.placements.is_empty() {
        return;
    }
    let Some(model_bytes) = prescan.model_bytes.as_deref() else {
        return;
    };

    let (model_min, model_max) = match glb_model_bbox(model_bytes) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "{} stadium GLB bbox failed: {e}",
                "Warning:".yellow().bold()
            );
            return;
        }
    };

    let mx = model_max[0] - model_min[0];
    let my = model_max[1] - model_min[1];
    let mz = model_max[2] - model_min[2];
    if mx < 1e-3 || my < 1e-3 || mz < 1e-3 {
        eprintln!(
            "{} stadium GLB has degenerate extents",
            "Warning:".yellow().bold()
        );
        return;
    }

    let model_x_is_long = mx >= mz;
    let model_long_extent = mx.max(mz);
    let model_short_extent = mx.min(mz);

    // Center XZ on origin; Y is set by the post-voxelize ground snap.
    let center_x = -(model_min[0] + model_max[0]) * 0.5;
    let center_z = -(model_min[2] + model_max[2]) * 0.5;

    println!(
        "{} Placing {} stadium model{}...",
        "  [+]".bold(),
        prescan.placements.len(),
        if prescan.placements.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    let block_per_meter = args.scale as f32;
    let mut placed = 0usize;
    let mut total_voxels = 0usize;

    for placement in &prescan.placements {
        let target_long_blocks = placement.long_m * block_per_meter;
        let target_short_blocks = placement.short_m * block_per_meter;
        let target_height_blocks = placement.osm_height_m.unwrap_or(DEFAULT_HEIGHT_M)
            * block_per_meter
            * HEIGHT_MULTIPLIER;

        // Pre-rotate Z-long models 90° so X is the effective long axis.
        let intrinsic_yaw_deg = if model_x_is_long { 0.0 } else { 90.0 };
        let scale_x = target_long_blocks / model_long_extent;
        let scale_z = target_short_blocks / model_short_extent;
        let scale_y = target_height_blocks / my;

        // Post-rotation offsets: rotating (-cx, *, -cz) by +90° yields (cz, *, -cx).
        let (intrinsic_tx, intrinsic_tz) = if model_x_is_long {
            (center_x, center_z)
        } else {
            (-center_z, center_x)
        };

        let fp = &placement.footprint;
        let ground_y =
            crate::models_3d::lowest_ground_in_bbox(editor, fp.min_x, fp.min_z, fp.max_x, fp.max_z);

        let transform = WorldTransform::with_world_scale_xyz(
            intrinsic_yaw_deg,
            1.0,
            [intrinsic_tx as f64, 0.0, intrinsic_tz as f64],
            [scale_x, scale_y, scale_z],
            placement.yaw_degrees,
            placement.anchor_x as f32,
            ground_y as f32,
            placement.anchor_z as f32,
        );

        let mut voxels = match voxelize_glb(model_bytes, transform) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "{} stadium (OSM {}) voxelization failed: {e}",
                    "Warning:".yellow().bold(),
                    placement.osm_id
                );
                continue;
            }
        };

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
    }

    println!(
        "  Placed {} stadium model{} ({} blocks)",
        placed.to_string().bright_white().bold(),
        if placed == 1 { "" } else { "s" },
        total_voxels
    );
}

fn polygon_points(element: &ProcessedElement) -> Option<Vec<(i32, i32)>> {
    let v: Vec<(i32, i32)> = match element {
        ProcessedElement::Way(w) => w.nodes.iter().map(|n| (n.x, n.z)).collect(),
        ProcessedElement::Relation(r) => r
            .members
            .iter()
            .flat_map(|m| m.way.nodes.iter().map(|n| (n.x, n.z)))
            .collect(),
        ProcessedElement::Node(_) => return None,
    };
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn anchor_xz(element: &ProcessedElement) -> Option<(i32, i32)> {
    match element {
        ProcessedElement::Node(n) => Some((n.x, n.z)),
        ProcessedElement::Way(w) => centroid_iter(w.nodes.iter().map(|n| (n.x, n.z))),
        ProcessedElement::Relation(r) => centroid_iter(
            r.members
                .iter()
                .flat_map(|m| m.way.nodes.iter().map(|n| (n.x, n.z))),
        ),
    }
}

fn centroid(points: &[(i32, i32)]) -> Option<(i32, i32)> {
    centroid_iter(points.iter().copied())
}

fn centroid_iter<I: Iterator<Item = (i32, i32)>>(coords: I) -> Option<(i32, i32)> {
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

fn bbox_of(points: &[(i32, i32)]) -> Option<Bbox> {
    let (x0, z0) = *points.first()?;
    let mut min_x = x0;
    let mut max_x = x0;
    let mut min_z = z0;
    let mut max_z = z0;
    for &(x, z) in &points[1..] {
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

/// PCA on (x, z): returns (long_extent, short_extent, theta_rad CCW from +X).
fn principal_axis(points: &[(i32, i32)]) -> Option<(f64, f64, f64)> {
    let n = points.len() as f64;
    if n < 3.0 {
        return None;
    }
    let cx = points.iter().map(|p| p.0 as f64).sum::<f64>() / n;
    let cz = points.iter().map(|p| p.1 as f64).sum::<f64>() / n;
    let mut cxx = 0.0_f64;
    let mut cxz = 0.0_f64;
    let mut czz = 0.0_f64;
    for &(x, z) in points {
        let dx = x as f64 - cx;
        let dz = z as f64 - cz;
        cxx += dx * dx;
        cxz += dx * dz;
        czz += dz * dz;
    }

    let theta = 0.5_f64 * (2.0 * cxz).atan2(cxx - czz);
    let (sin_t, cos_t) = theta.sin_cos();

    let mut min_a = f64::INFINITY;
    let mut max_a = f64::NEG_INFINITY;
    let mut min_p = f64::INFINITY;
    let mut max_p = f64::NEG_INFINITY;
    for &(x, z) in points {
        let dx = x as f64 - cx;
        let dz = z as f64 - cz;
        let a = dx * cos_t + dz * sin_t;
        let p = -dx * sin_t + dz * cos_t;
        if a < min_a {
            min_a = a;
        }
        if a > max_a {
            max_a = a;
        }
        if p < min_p {
            min_p = p;
        }
        if p > max_p {
            max_p = p;
        }
    }
    let ext_a = max_a - min_a;
    let ext_p = max_p - min_p;
    if ext_a >= ext_p {
        Some((ext_a, ext_p, theta))
    } else {
        Some((ext_p, ext_a, theta + std::f64::consts::FRAC_PI_2))
    }
}

fn parse_meters(raw: &str) -> Option<f64> {
    let s = raw.trim().trim_end_matches('m').trim();
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};
    use std::collections::HashMap as StdMap;

    fn mk_node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: StdMap::new(),
            x,
            z,
        }
    }

    fn mk_way(id: u64, nodes: Vec<ProcessedNode>, tags: StdMap<String, String>) -> ProcessedWay {
        ProcessedWay { id, nodes, tags }
    }

    #[test]
    fn principal_axis_of_axis_aligned_rect() {
        let pts = vec![(-100, -50), (100, -50), (100, 50), (-100, 50)];
        let (long, short, theta) = principal_axis(&pts).unwrap();
        assert!((long - 200.0).abs() < 1e-6);
        assert!((short - 100.0).abs() < 1e-6);
        assert!(theta.abs() < 1e-6, "theta = {theta}");
    }

    #[test]
    fn principal_axis_of_z_aligned_rect_returns_long_first() {
        let pts = vec![(-50, -100), (50, -100), (50, 100), (-50, 100)];
        let (long, short, theta) = principal_axis(&pts).unwrap();
        assert!((long - 200.0).abs() < 1e-6);
        assert!((short - 100.0).abs() < 1e-6);
        let t = theta.rem_euclid(std::f64::consts::PI);
        assert!(
            (t - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "theta = {theta}"
        );
    }

    #[test]
    fn principal_axis_of_45deg_rect() {
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        let pts: Vec<(i32, i32)> = [
            (-100.0, -50.0),
            (100.0, -50.0),
            (100.0, 50.0),
            (-100.0, 50.0),
        ]
        .iter()
        .map(|&(x, z)| {
            let rx = x * s2 - z * s2;
            let rz = x * s2 + z * s2;
            (rx.round() as i32, rz.round() as i32)
        })
        .collect();
        let (long, short, theta) = principal_axis(&pts).unwrap();
        assert!((long - 200.0).abs() < 2.0, "long = {long}");
        assert!((short - 100.0).abs() < 2.0, "short = {short}");
        let t = theta.rem_euclid(std::f64::consts::PI);
        assert!(
            (t - std::f64::consts::FRAC_PI_4).abs() < 0.05,
            "theta = {theta}"
        );
    }

    fn prescan_offline(
        elements: &[ProcessedElement],
        already: &HashSet<(&'static str, u64)>,
    ) -> (Vec<Placement>, HashSet<(&'static str, u64)>) {
        let (placements, mut suppressed, footprints) =
            collect_stadium_placements(elements, already, 1.0);
        let interior = collect_interior_suppression(elements, already, &suppressed, &footprints);
        suppressed.extend(interior);
        (placements, suppressed)
    }

    #[test]
    fn prescan_claims_leisure_stadium_and_suppresses_inner_pitch() {
        let mut stadium_tags = StdMap::new();
        stadium_tags.insert("leisure".to_string(), "stadium".to_string());
        let stadium = ProcessedElement::Way(mk_way(
            1,
            vec![
                mk_node(10, -150, -100),
                mk_node(11, 150, -100),
                mk_node(12, 150, 100),
                mk_node(13, -150, 100),
            ],
            stadium_tags,
        ));

        let mut pitch_tags = StdMap::new();
        pitch_tags.insert("leisure".to_string(), "pitch".to_string());
        let pitch = ProcessedElement::Way(mk_way(
            2,
            vec![
                mk_node(20, -40, -20),
                mk_node(21, 40, -20),
                mk_node(22, 40, 20),
                mk_node(23, -40, 20),
            ],
            pitch_tags,
        ));

        let mut gs_tags = StdMap::new();
        gs_tags.insert("building".to_string(), "grandstand".to_string());
        let grandstand = ProcessedElement::Way(mk_way(
            3,
            vec![
                mk_node(30, -120, -90),
                mk_node(31, -100, -90),
                mk_node(32, -100, 90),
                mk_node(33, -120, 90),
            ],
            gs_tags,
        ));

        let mut road_tags = StdMap::new();
        road_tags.insert("highway".to_string(), "service".to_string());
        let road = ProcessedElement::Way(mk_way(
            4,
            vec![mk_node(40, -10, -10), mk_node(41, 10, 10)],
            road_tags,
        ));

        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[stadium, pitch, grandstand, road], &empty);

        assert_eq!(placements.len(), 1);
        assert!(suppressed.contains(&("way", 1)));
        assert!(suppressed.contains(&("way", 2)));
        assert!(suppressed.contains(&("way", 3)));
        assert!(!suppressed.contains(&("way", 4)));
    }

    #[test]
    fn prescan_falls_back_to_building_stadium_when_no_leisure() {
        let mut bs_tags = StdMap::new();
        bs_tags.insert("building".to_string(), "stadium".to_string());
        let bs = ProcessedElement::Way(mk_way(
            5,
            vec![
                mk_node(50, 0, 0),
                mk_node(51, 200, 0),
                mk_node(52, 200, 150),
                mk_node(53, 0, 150),
            ],
            bs_tags,
        ));

        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[bs], &empty);
        assert_eq!(placements.len(), 1);
        assert!(suppressed.contains(&("way", 5)));
    }

    #[test]
    fn prescan_subsumes_building_stadium_inside_leisure_stadium() {
        let mut ls_tags = StdMap::new();
        ls_tags.insert("leisure".to_string(), "stadium".to_string());
        let ls = ProcessedElement::Way(mk_way(
            6,
            vec![
                mk_node(60, -200, -150),
                mk_node(61, 200, -150),
                mk_node(62, 200, 150),
                mk_node(63, -200, 150),
            ],
            ls_tags,
        ));

        let mut bs_tags = StdMap::new();
        bs_tags.insert("building".to_string(), "stadium".to_string());
        let bs = ProcessedElement::Way(mk_way(
            7,
            vec![
                mk_node(70, -100, -100),
                mk_node(71, 100, -100),
                mk_node(72, 100, 100),
                mk_node(73, -100, 100),
            ],
            bs_tags,
        ));

        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[ls, bs], &empty);
        assert_eq!(placements.len(), 1);
        assert!(suppressed.contains(&("way", 6)));
        assert!(suppressed.contains(&("way", 7)));
    }

    #[test]
    fn prescan_rejects_tiny_stadium() {
        let mut tags = StdMap::new();
        tags.insert("leisure".to_string(), "stadium".to_string());
        let tiny = ProcessedElement::Way(mk_way(
            8,
            vec![
                mk_node(80, 0, 0),
                mk_node(81, 8, 0),
                mk_node(82, 8, 6),
                mk_node(83, 0, 6),
            ],
            tags,
        ));
        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[tiny], &empty);
        assert_eq!(placements.len(), 0);
        assert!(!suppressed.contains(&("way", 8)));
    }

    #[test]
    fn prescan_rejects_medium_stadium_without_inner_building() {
        let mut tags = StdMap::new();
        tags.insert("leisure".to_string(), "stadium".to_string());
        tags.insert("sport".to_string(), "swimming".to_string());
        let swim = ProcessedElement::Way(mk_way(
            9,
            vec![
                mk_node(90, -55, -50),
                mk_node(91, 55, -50),
                mk_node(92, 55, 50),
                mk_node(93, -55, 50),
            ],
            tags,
        ));
        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[swim], &empty);
        assert_eq!(placements.len(), 0);
        assert!(!suppressed.contains(&("way", 9)));
    }

    #[test]
    fn prescan_rejects_small_stadium_even_with_inner_building() {
        let mut ls_tags = StdMap::new();
        ls_tags.insert("leisure".to_string(), "stadium".to_string());
        let ls = ProcessedElement::Way(mk_way(
            10,
            vec![
                mk_node(100, -60, -30),
                mk_node(101, 60, -30),
                mk_node(102, 60, 30),
                mk_node(103, -60, 30),
            ],
            ls_tags,
        ));

        let mut bs_tags = StdMap::new();
        bs_tags.insert("building".to_string(), "stadium".to_string());
        let bs = ProcessedElement::Way(mk_way(
            11,
            vec![
                mk_node(110, -50, -25),
                mk_node(111, 50, -25),
                mk_node(112, 50, 25),
                mk_node(113, -50, 25),
            ],
            bs_tags,
        ));

        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[ls, bs], &empty);
        assert_eq!(placements.len(), 0);
        assert!(!suppressed.contains(&("way", 10)));
        assert!(!suppressed.contains(&("way", 11)));
    }

    #[test]
    fn prescan_accepts_medium_stadium_with_inner_building() {
        let mut ls_tags = StdMap::new();
        ls_tags.insert("leisure".to_string(), "stadium".to_string());
        let ls = ProcessedElement::Way(mk_way(
            12,
            vec![
                mk_node(120, -65, -50),
                mk_node(121, 65, -50),
                mk_node(122, 65, 50),
                mk_node(123, -65, 50),
            ],
            ls_tags,
        ));

        let mut bs_tags = StdMap::new();
        bs_tags.insert("building".to_string(), "stadium".to_string());
        let bs = ProcessedElement::Way(mk_way(
            13,
            vec![
                mk_node(130, -55, -40),
                mk_node(131, 55, -40),
                mk_node(132, 55, 40),
                mk_node(133, -55, 40),
            ],
            bs_tags,
        ));

        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[ls, bs], &empty);
        assert_eq!(placements.len(), 1);
        assert!(suppressed.contains(&("way", 12)));
        assert!(suppressed.contains(&("way", 13)));
    }

    #[test]
    fn prescan_rejects_oversize_stadium() {
        // 1500×800 m — likely a whole sports complex mis-tagged. Above MAX_LONG_EXTENT_M.
        let mut tags = StdMap::new();
        tags.insert("leisure".to_string(), "stadium".to_string());
        let huge = ProcessedElement::Way(mk_way(
            20,
            vec![
                mk_node(200, 0, 0),
                mk_node(201, 1500, 0),
                mk_node(202, 1500, 800),
                mk_node(203, 0, 800),
            ],
            tags,
        ));
        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[huge], &empty);
        assert_eq!(placements.len(), 0);
        assert!(!suppressed.contains(&("way", 20)));
    }

    #[test]
    fn prescan_rejects_standalone_small_building_stadium() {
        let mut tags = StdMap::new();
        tags.insert("building".to_string(), "stadium".to_string());
        let small = ProcessedElement::Way(mk_way(
            14,
            vec![
                mk_node(140, 0, 0),
                mk_node(141, 100, 0),
                mk_node(142, 100, 80),
                mk_node(143, 0, 80),
            ],
            tags,
        ));
        let empty = HashSet::new();
        let (placements, suppressed) = prescan_offline(&[small], &empty);
        assert_eq!(placements.len(), 0);
        assert!(!suppressed.contains(&("way", 14)));
    }

    /// Berlin Olympiapark regression: only the real Olympiastadion claims a model.
    #[test]
    fn prescan_berlin_olympiapark_only_olympiastadion_claims_model() {
        let mut olympia_tags = StdMap::new();
        olympia_tags.insert("leisure".to_string(), "stadium".to_string());
        olympia_tags.insert("name".to_string(), "Olympiastadion Berlin".to_string());
        let olympia = ProcessedElement::Way(mk_way(
            38862723,
            vec![
                mk_node(1000, -125, -110),
                mk_node(1001, 125, -110),
                mk_node(1002, 125, 110),
                mk_node(1003, -125, 110),
            ],
            olympia_tags,
        ));
        let mut olympia_bs_tags = StdMap::new();
        olympia_bs_tags.insert("building".to_string(), "stadium".to_string());
        let olympia_bs = ProcessedElement::Way(mk_way(
            24296022,
            vec![
                mk_node(1010, -115, -100),
                mk_node(1011, 115, -100),
                mk_node(1012, 115, 100),
                mk_node(1013, -115, 100),
            ],
            olympia_bs_tags,
        ));

        let mut swim_tags = StdMap::new();
        swim_tags.insert("leisure".to_string(), "stadium".to_string());
        swim_tags.insert("name".to_string(), "Olympia-Schwimmstadion".to_string());
        let swim = ProcessedElement::Way(mk_way(
            38863016,
            vec![
                mk_node(2000, 800, -40),
                mk_node(2001, 900, -40),
                mk_node(2002, 900, 40),
                mk_node(2003, 800, 40),
            ],
            swim_tags,
        ));

        let mut amateur_tags = StdMap::new();
        amateur_tags.insert("leisure".to_string(), "stadium".to_string());
        amateur_tags.insert("name".to_string(), "Stadion auf dem Wurfplatz".to_string());
        let amateur = ProcessedElement::Way(mk_way(
            24296069,
            vec![
                mk_node(3000, -1500, -30),
                mk_node(3001, -1380, -30),
                mk_node(3002, -1380, 30),
                mk_node(3003, -1500, 30),
            ],
            amateur_tags,
        ));
        let mut amateur_bs_tags = StdMap::new();
        amateur_bs_tags.insert("building".to_string(), "stadium".to_string());
        let amateur_bs = ProcessedElement::Way(mk_way(
            764233954,
            vec![
                mk_node(3010, -1490, -25),
                mk_node(3011, -1390, -25),
                mk_node(3012, -1390, 25),
                mk_node(3013, -1490, 25),
            ],
            amateur_bs_tags,
        ));

        let empty = HashSet::new();
        let (placements, suppressed) =
            prescan_offline(&[olympia, olympia_bs, swim, amateur, amateur_bs], &empty);

        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].osm_id, 38862723);

        assert!(suppressed.contains(&("way", 38862723)));
        assert!(suppressed.contains(&("way", 24296022)));

        assert!(!suppressed.contains(&("way", 38863016)));
        assert!(!suppressed.contains(&("way", 24296069)));
        assert!(!suppressed.contains(&("way", 764233954)));
    }

    #[test]
    fn parse_meters_basic() {
        assert_eq!(parse_meters("12"), Some(12.0));
        assert_eq!(parse_meters("12.5"), Some(12.5));
        assert_eq!(parse_meters("28 m"), Some(28.0));
        assert_eq!(parse_meters("28m"), Some(28.0));
        assert_eq!(parse_meters("bogus"), None);
        assert_eq!(parse_meters("-3"), None);
    }
}
