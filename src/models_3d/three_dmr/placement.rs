//! 3DMR pre-scan, suppression set, and end-to-end placement orchestration.

use crate::args::Args;
use crate::models_3d::three_dmr::client::{fetch_glb, fetch_info, ModelInfo};
use crate::models_3d::voxelize::{voxelize_glb, WorldTransform};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
struct Placement {
    osm_id: u64,
    model_id: u64,
    anchor_x: i32,
    anchor_z: i32,
    footprint: Bbox,
    world_yaw_degrees: f64,
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
}

/// Assumed max XZ half-extent (m) for the otherwise-uncapped 3DMR models, so their regions can
/// be deferred under stream-to-disk. Generous; a model exceeding this (+512-block ring) truncates.
const ASSUMED_HALF_EXTENT_M: f64 = 384.0;

impl PrescanResult {
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Regions each placement may write to (stream-to-disk deferral); see ASSUMED_HALF_EXTENT_M.
    pub fn deferred_region_keys(&self, scale: f64) -> Vec<(i32, i32)> {
        let r = (ASSUMED_HALF_EXTENT_M * scale).ceil() as i32;
        self.placements
            .iter()
            .flat_map(|p| crate::models_3d::region_keys_around(p.anchor_x, p.anchor_z, r))
            .collect()
    }
}

/// Scans for 3dmr=<id> tags; suppresses the tagged element plus any building-like element inside its footprint.
pub fn prescan(elements: &[ProcessedElement], world_rotation: f64) -> PrescanResult {
    let mut placements = Vec::new();
    let mut suppressed = HashSet::new();
    let mut footprints: Vec<Bbox> = Vec::new();

    for element in elements {
        let Some(id_str) = element.tags().get("3dmr") else {
            continue;
        };
        let Some(model_id) = parse_model_id(id_str) else {
            continue;
        };
        let Some((anchor_x, anchor_z)) = anchor_xz(element) else {
            continue;
        };

        let osm_yaw = element
            .tags()
            .get("direction")
            .and_then(|s| parse_direction(s))
            .unwrap_or(0.0);
        let world_yaw_degrees = osm_yaw + world_rotation;

        // Synthesize a small bbox for node anchors so the lowest-Y sweep has something to scan.
        let raw_footprint = osm_bbox(element);
        let footprint = raw_footprint.unwrap_or_else(|| Bbox {
            min_x: anchor_x - 8,
            max_x: anchor_x + 8,
            min_z: anchor_z - 8,
            max_z: anchor_z + 8,
        });

        suppressed.insert((element.kind(), element.id()));
        if let Some(bbox) = raw_footprint {
            footprints.push(bbox);
        }
        placements.push(Placement {
            osm_id: element.id(),
            model_id,
            anchor_x,
            anchor_z,
            footprint,
            world_yaw_degrees,
        });
    }

    if !footprints.is_empty() {
        for element in elements {
            let key = (element.kind(), element.id());
            if suppressed.contains(&key) {
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
    }
}

/// Fetches models (with cache), voxelizes them, and writes blocks into the editor.
pub fn place_three_dmr_models(editor: &mut WorldEditor, args: &Args, prescan: &PrescanResult) {
    if prescan.placements.is_empty() {
        return;
    }

    println!(
        "{} Fetching {} 3DMR model{}...",
        "  [+]".bold(),
        prescan.placements.len(),
        if prescan.placements.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    let unique_ids: Vec<u64> = {
        let mut set: HashSet<u64> = HashSet::new();
        prescan.placements.iter().for_each(|p| {
            set.insert(p.model_id);
        });
        set.into_iter().collect()
    };

    let fetched: HashMap<u64, (Vec<u8>, ModelInfo)> = unique_ids
        .par_iter()
        .filter_map(|id| {
            let info = match fetch_info(*id) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!(
                        "{} 3DMR info {id} fetch failed: {e}",
                        "Warning:".yellow().bold()
                    );
                    return None;
                }
            };
            match fetch_glb(*id) {
                Ok(bytes) => Some((*id, (bytes, info))),
                Err(e) => {
                    eprintln!(
                        "{} 3DMR model {id} fetch failed: {e}",
                        "Warning:".yellow().bold()
                    );
                    None
                }
            }
        })
        .collect();

    let scale = args.scale;
    let mut placed_count = 0usize;
    let mut total_voxels = 0usize;

    for placement in &prescan.placements {
        let Some((glb_bytes, info)) = fetched.get(&placement.model_id) else {
            continue;
        };

        let fp = &placement.footprint;
        let ground_y =
            crate::models_3d::lowest_ground_in_bbox(editor, fp.min_x, fp.min_z, fp.max_x, fp.max_z);

        let transform = WorldTransform::new(
            info.rotation,
            info.scale,
            info.translation,
            scale,
            placement.world_yaw_degrees,
            placement.anchor_x as f32,
            ground_y as f32,
            placement.anchor_z as f32,
        );

        let mut voxels = match voxelize_glb(glb_bytes, transform) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "{} 3DMR model {} (OSM {}) voxelization failed: {e}",
                    "Warning:".yellow().bold(),
                    placement.model_id,
                    placement.osm_id
                );
                continue;
            }
        };

        // Snap the model's lowest voxel onto the lowest ground Y in the footprint.
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
        placed_count += 1;

        let title = info.title.as_deref().unwrap_or("(untitled)");
        let author = info.author.as_deref().unwrap_or("unknown");
        let license = info.license.as_deref().unwrap_or("unspecified");
        println!(
            "    3DMR {} — \"{title}\" by {author} (license {license})",
            placement.model_id
        );
    }

    println!(
        "  Placed {} 3DMR model{} ({} blocks)",
        placed_count.to_string().bright_white().bold(),
        if placed_count == 1 { "" } else { "s" },
        total_voxels
    );
}

fn parse_model_id(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok()
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

/// Parses OSM `direction=*` (numeric degrees or cardinal abbrev) → degrees CW from north.
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
    fn parses_numeric_direction() {
        assert_eq!(parse_direction("0"), Some(0.0));
        assert_eq!(parse_direction("180"), Some(180.0));
        assert_eq!(parse_direction(" 270 "), Some(270.0));
        assert_eq!(parse_direction("450"), Some(90.0));
    }

    #[test]
    fn parses_cardinal_direction() {
        assert_eq!(parse_direction("N"), Some(0.0));
        assert_eq!(parse_direction("east"), Some(90.0));
        assert_eq!(parse_direction("SW"), Some(225.0));
        assert_eq!(parse_direction("nnw"), Some(337.5));
        assert_eq!(parse_direction("bogus"), None);
    }

    #[test]
    fn parses_model_id() {
        assert_eq!(parse_model_id("42"), Some(42));
        assert_eq!(parse_model_id(" 7 "), Some(7));
        assert_eq!(parse_model_id(""), None);
        assert_eq!(parse_model_id("not-a-number"), None);
    }

    #[test]
    fn centroid_of_square() {
        let pts = [(0, 0), (10, 0), (10, 10), (0, 10)].into_iter();
        assert_eq!(centroid(pts), Some((5, 5)));
    }

    #[test]
    fn bbox_contains_inside_outside() {
        let b = Bbox {
            min_x: 0,
            min_z: 0,
            max_x: 10,
            max_z: 10,
        };
        assert!(b.contains(5, 5));
        assert!(b.contains(0, 0));
        assert!(b.contains(10, 10));
        assert!(!b.contains(-1, 5));
        assert!(!b.contains(5, 11));
    }

    #[test]
    fn prescan_suppresses_building_inside_3dmr_footprint() {
        use crate::osm_parser::{ProcessedNode, ProcessedWay};
        use std::collections::HashMap as StdMap;

        let mk_node = |id: u64, x: i32, z: i32| ProcessedNode {
            id,
            tags: StdMap::new(),
            x,
            z,
        };
        let mk_way = |id: u64, nodes: Vec<ProcessedNode>, tags: StdMap<String, String>| {
            ProcessedWay { id, nodes, tags }
        };

        // 3DMR-tagged way: square (0..100, 0..100)
        let mut tagged_tags = StdMap::new();
        tagged_tags.insert("3dmr".to_string(), "42".to_string());
        tagged_tags.insert("building".to_string(), "yes".to_string());
        let tagged = ProcessedElement::Way(mk_way(
            1,
            vec![
                mk_node(10, 0, 0),
                mk_node(11, 100, 0),
                mk_node(12, 100, 100),
                mk_node(13, 0, 100),
            ],
            tagged_tags,
        ));

        // Building inside the footprint — should be suppressed by spatial pass.
        let mut inside_tags = StdMap::new();
        inside_tags.insert("building".to_string(), "yes".to_string());
        let inside = ProcessedElement::Way(mk_way(
            2,
            vec![
                mk_node(20, 40, 40),
                mk_node(21, 60, 40),
                mk_node(22, 60, 60),
                mk_node(23, 40, 60),
            ],
            inside_tags,
        ));

        // Building well outside — must not be suppressed.
        let mut outside_tags = StdMap::new();
        outside_tags.insert("building".to_string(), "yes".to_string());
        let outside = ProcessedElement::Way(mk_way(
            3,
            vec![
                mk_node(30, 500, 500),
                mk_node(31, 510, 500),
                mk_node(32, 510, 510),
                mk_node(33, 500, 510),
            ],
            outside_tags,
        ));

        // A road inside the footprint — must not be suppressed (only buildings).
        let mut road_tags = StdMap::new();
        road_tags.insert("highway".to_string(), "service".to_string());
        let road = ProcessedElement::Way(mk_way(
            4,
            vec![mk_node(40, 20, 20), mk_node(41, 80, 80)],
            road_tags,
        ));

        let result = prescan(&[tagged, inside, outside, road], 0.0);
        assert!(
            result.suppressed_ids.contains(&("way", 1)),
            "tagged element suppressed"
        );
        assert!(
            result.suppressed_ids.contains(&("way", 2)),
            "building inside footprint suppressed"
        );
        assert!(
            !result.suppressed_ids.contains(&("way", 3)),
            "building outside footprint NOT suppressed"
        );
        assert!(
            !result.suppressed_ids.contains(&("way", 4)),
            "road NOT suppressed"
        );
        assert_eq!(result.placement_count(), 1);
    }
}
