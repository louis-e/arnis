//! Aeroplane archetype: planes parked on runways / long straight taxiways, plus one climbing off
//! the end of long runways.
//!
//! OSM splits runways and taxiways (`aeroway=runway`/`taxiway`) into multiple centerline ways with
//! no relation linking them, so we merge same-kind segments by shared end nodes + near-collinear
//! bearing and treat each merged run as one strip. Aprons/gates (where planes really park) are
//! polygons, not centerlines, so taxiway parking on straight runs is the next-best proxy.

use crate::args::Args;
use crate::deterministic_rng::element_rng;
use crate::models_3d::custom::client;
use crate::models_3d::voxelize::{glb_model_bbox, voxelize_glb, WorldTransform};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use rand::Rng;
use std::collections::HashMap;

const MODEL_URL: &str = "https://arnismc.com/assets/3dmodels/plane.glb";
const CACHE_FILE: &str = "plane.glb";

/// Length the plane is voxelized to. Asset convention: nose-tail +Z, wingspan X, up Y; uniform scale.
const PLANE_LENGTH_M: f64 = 90.0;
/// Nose-up tilt for the climbing-out plane.
const ASCENDING_PITCH_DEG: f64 = 12.0;
/// Runways at least this long always get a plane climbing off one end.
const ASCENDING_MIN_LENGTH_M: f64 = 1500.0;
/// Climb-out height above ground = this fraction of a plane-length, plus `ASCENDING_EXTRA_ELEV_M`.
const ASCENDING_ELEV_FACTOR: f64 = 0.45;
/// Flat extra climb-out height (metres) on top of the proportional part.
const ASCENDING_EXTRA_ELEV_M: f64 = 20.0;
/// Per-runway chance of a plane parked on the centerline.
const RUNWAY_PARK_PROBABILITY: f64 = 0.4;
/// Per-taxiway chance of a taxiing plane — lower than runways, which are far fewer.
const TAXIWAY_PARK_PROBABILITY: f64 = 0.15;
/// A parked plane needs a strip at least this long so it sits fully between the ends.
const PARKED_MIN_LENGTH_M: f64 = 120.0;
/// Above this length an aeroway is almost certainly mis-tagged (a whole airfield); skip it.
const MAX_AEROWAY_LENGTH_M: f64 = 8000.0;
/// Two segments sharing an end node merge only if their bearings differ by less than this.
const COLLINEAR_TOL_RAD: f64 = 0.349; // ~20°
/// Straightness cap (perpendicular / long extent); stricter for curvy taxiways than runways.
const RUNWAY_MAX_PERP_RATIO: f64 = 0.5;
const TAXIWAY_MAX_PERP_RATIO: f64 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlaneKind {
    Parked,
    Ascending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AerowayKind {
    Runway,
    Taxiway,
}

#[derive(Clone, Copy, Debug)]
struct Bbox {
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
}

#[derive(Clone, Debug)]
struct Placement {
    rep_id: u64,
    kind: PlaneKind,
    anchor_x: i32,
    anchor_z: i32,
    /// World yaw (degrees) that aligns the model's +Z nose with the aeroway direction.
    yaw_degrees: f64,
    pitch_degrees: f64,
    elevation_blocks: i32,
    footprint: Bbox,
}

pub struct PrescanResult {
    placements: Vec<Placement>,
    model_bytes: Option<Vec<u8>>,
}

impl PrescanResult {
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Regions each placement may write to (stream-to-disk deferral). The model spans at
    /// most PLANE_LENGTH_M from the anchor, so that radius (+ ring) is a safe superset.
    pub fn deferred_region_keys(&self, scale: f64) -> Vec<(i32, i32)> {
        let r = (PLANE_LENGTH_M * scale).ceil() as i32;
        self.placements
            .iter()
            .flat_map(|p| crate::models_3d::region_keys_around(p.anchor_x, p.anchor_z, r))
            .collect()
    }
}

pub fn prescan(elements: &[ProcessedElement], args_scale: f64) -> PrescanResult {
    let aeroways = collect_aeroways(elements);
    let placements = build_placements(&aeroways, args_scale);
    if placements.is_empty() {
        return PrescanResult {
            placements,
            model_bytes: None,
        };
    }

    match client::fetch_glb(MODEL_URL, CACHE_FILE) {
        Ok(b) => PrescanResult {
            placements,
            model_bytes: Some(b),
        },
        Err(e) => {
            eprintln!(
                "{} plane model fetch failed ({MODEL_URL}): {e}",
                "Warning:".yellow().bold()
            );
            PrescanResult {
                placements: Vec::new(),
                model_bytes: None,
            }
        }
    }
}

pub fn place_plane_models(editor: &mut WorldEditor, args: &Args, prescan: &PrescanResult) {
    if prescan.placements.is_empty() {
        return;
    }
    let Some(model_bytes) = prescan.model_bytes.as_deref() else {
        return;
    };

    let (model_min, model_max) = match glb_model_bbox(model_bytes) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{} plane GLB bbox failed: {e}", "Warning:".yellow().bold());
            return;
        }
    };
    // Nose-tail runs along +Z in the asset; uniform scale keeps the plane's proportions.
    let model_len = model_max[2] - model_min[2];
    if model_len < 1e-3 {
        eprintln!(
            "{} plane GLB has degenerate length",
            "Warning:".yellow().bold()
        );
        return;
    }
    let target_len_blocks = (PLANE_LENGTH_M * args.scale) as f32;
    let intrinsic_scale = (target_len_blocks / model_len) as f64;

    println!(
        "{} Placing {} plane model{}...",
        "  [+]".bold(),
        prescan.placements.len(),
        if prescan.placements.len() == 1 {
            ""
        } else {
            "s"
        }
    );

    let mut parked = 0usize;
    let mut climbing = 0usize;
    let mut total_voxels = 0usize;

    for p in &prescan.placements {
        let fp = &p.footprint;
        let ground_y =
            crate::models_3d::lowest_ground_in_bbox(editor, fp.min_x, fp.min_z, fp.max_x, fp.max_z);

        let transform = WorldTransform::new(
            0.0,
            intrinsic_scale,
            [0.0, 0.0, 0.0],
            1.0,
            p.yaw_degrees,
            p.anchor_x as f32,
            ground_y as f32,
            p.anchor_z as f32,
        )
        .pitched(p.pitch_degrees);

        let mut voxels = match voxelize_glb(model_bytes, transform) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "{} plane (OSM {}) voxelization failed: {e}",
                    "Warning:".yellow().bold(),
                    p.rep_id
                );
                continue;
            }
        };

        // Lift so the lowest voxel rests at ground + elevation (0 = wheels on the runway).
        if let Some(min_y) = voxels.iter().map(|(q, _)| q[1]).min() {
            let dy = (ground_y + p.elevation_blocks) - min_y;
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
        match p.kind {
            PlaneKind::Parked => parked += 1,
            PlaneKind::Ascending => climbing += 1,
        }
    }

    println!(
        "  Placed {} plane model{} ({parked} parked, {climbing} climbing; {total_voxels} blocks)",
        (parked + climbing).to_string().bright_white().bold(),
        if parked + climbing == 1 { "" } else { "s" },
    );
}

// ---------------------------------------------------------------------------
// Aeroway extraction + merging
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Seg {
    way_id: u64,
    kind: AerowayKind,
    first_node: u64,
    last_node: u64,
    points: Vec<(f64, f64)>,
    /// Undirected bearing in [0, π).
    angle: f64,
}

#[derive(Clone, Debug)]
struct AerowayStrip {
    /// Smallest OSM way id in the merged group — a stable seed for deterministic placement.
    rep_id: u64,
    kind: AerowayKind,
    centroid: (f64, f64),
    /// Unit vector along the long axis.
    dir: (f64, f64),
    length_blocks: f64,
    perp_blocks: f64,
    /// Min/max projection of the points onto `dir`, measured from `centroid`.
    min_a: f64,
    max_a: f64,
}

fn collect_aeroways(elements: &[ProcessedElement]) -> Vec<AerowayStrip> {
    let segs: Vec<Seg> = elements
        .iter()
        .filter_map(extract_aeroway_segment)
        .collect();
    if segs.is_empty() {
        return Vec::new();
    }

    let mut uf = UnionFind::new(segs.len());
    let mut endpoint_map: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        endpoint_map.entry(s.first_node).or_default().push(i);
        endpoint_map.entry(s.last_node).or_default().push(i);
    }
    for ids in endpoint_map.values() {
        for a in 0..ids.len() {
            for b in (a + 1)..ids.len() {
                let (ia, ib) = (ids[a], ids[b]);
                // Same kind only — a taxiway meeting a runway end-on must not fuse.
                if segs[ia].kind == segs[ib].kind && collinear(segs[ia].angle, segs[ib].angle) {
                    uf.union(ia, ib);
                }
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..segs.len() {
        groups.entry(uf.find(i)).or_default().push(i);
    }
    groups
        .values()
        .filter_map(|g| strip_from_group(&segs, g))
        .collect()
}

fn extract_aeroway_segment(element: &ProcessedElement) -> Option<Seg> {
    let ProcessedElement::Way(w) = element else {
        return None;
    };
    let kind = match w.tags.get("aeroway").map(|s| s.as_str()) {
        Some("runway") => AerowayKind::Runway,
        Some("taxiway") => AerowayKind::Taxiway,
        _ => return None,
    };
    // Skip area representations — we only place planes along linear centerlines.
    if w.tags.get("area").map(|s| s.as_str()) == Some("yes") {
        return None;
    }
    if w.nodes.len() < 2 {
        return None;
    }
    let first = &w.nodes[0];
    let last = &w.nodes[w.nodes.len() - 1];
    if first.id == last.id {
        return None; // closed loop = area, not a centerline
    }
    let dx = last.x as f64 - first.x as f64;
    let dz = last.z as f64 - first.z as f64;
    if dx == 0.0 && dz == 0.0 {
        return None;
    }
    Some(Seg {
        way_id: w.id,
        kind,
        first_node: first.id,
        last_node: last.id,
        points: w.nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect(),
        angle: dz.atan2(dx).rem_euclid(std::f64::consts::PI),
    })
}

/// True when two undirected bearings in [0, π) are within `COLLINEAR_TOL_RAD`.
fn collinear(a: f64, b: f64) -> bool {
    let d = (a - b).abs();
    d.min(std::f64::consts::PI - d) < COLLINEAR_TOL_RAD
}

fn strip_from_group(segs: &[Seg], group: &[usize]) -> Option<AerowayStrip> {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let mut rep_id = u64::MAX;
    for &i in group {
        rep_id = rep_id.min(segs[i].way_id);
        pts.extend_from_slice(&segs[i].points);
    }
    // All segments in a merged group share a kind (the merge rule enforces it).
    let kind = segs[group[0]].kind;
    principal_geom(&pts, rep_id, kind)
}

/// PCA on the merged point cloud: long-axis direction, length, perpendicular extent.
fn principal_geom(points: &[(f64, f64)], rep_id: u64, kind: AerowayKind) -> Option<AerowayStrip> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let cx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let cz = points.iter().map(|p| p.1).sum::<f64>() / n;
    let (mut cxx, mut cxz, mut czz) = (0.0_f64, 0.0_f64, 0.0_f64);
    for &(x, z) in points {
        let dx = x - cx;
        let dz = z - cz;
        cxx += dx * dx;
        cxz += dx * dz;
        czz += dz * dz;
    }

    let theta = 0.5 * (2.0 * cxz).atan2(cxx - czz);
    let (mut s, mut c) = theta.sin_cos();
    let extent = |s: f64, c: f64| {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &(x, z) in points {
            let v = (x - cx) * c + (z - cz) * s;
            lo = lo.min(v);
            hi = hi.max(v);
        }
        (lo, hi)
    };

    let (mut min_a, mut max_a) = extent(s, c);
    let (mut min_p, mut max_p) = extent(c, -s); // perpendicular axis
                                                // `theta` may land on the short axis; flip to the longer one so `dir` is nose-tail.
    if (max_p - min_p) > (max_a - min_a) {
        let (ns, nc) = (c, -s);
        s = ns;
        c = nc;
        std::mem::swap(&mut min_a, &mut min_p);
        std::mem::swap(&mut max_a, &mut max_p);
    }

    let length = max_a - min_a;
    if length <= 0.0 {
        return None;
    }
    Some(AerowayStrip {
        rep_id,
        kind,
        centroid: (cx, cz),
        dir: (c, s),
        length_blocks: length,
        perp_blocks: max_p - min_p,
        min_a,
        max_a,
    })
}

// ---------------------------------------------------------------------------
// Placement decisions
// ---------------------------------------------------------------------------

fn build_placements(strips: &[AerowayStrip], scale: f64) -> Vec<Placement> {
    let mut out = Vec::new();
    let plane_len_blocks = PLANE_LENGTH_M * scale;

    for strip in strips {
        let length_m = strip.length_blocks / scale;
        let perp_ratio = strip.perp_blocks / strip.length_blocks;
        let (park_probability, max_perp_ratio) = match strip.kind {
            AerowayKind::Runway => (RUNWAY_PARK_PROBABILITY, RUNWAY_MAX_PERP_RATIO),
            AerowayKind::Taxiway => (TAXIWAY_PARK_PROBABILITY, TAXIWAY_MAX_PERP_RATIO),
        };
        if length_m > MAX_AEROWAY_LENGTH_M || perp_ratio > max_perp_ratio {
            continue;
        }
        let yaw = yaw_for_dir(strip.dir);
        let mut rng = element_rng(strip.rep_id);

        // Climbing plane: runways only (nothing takes off from a taxiway), off the far end.
        if strip.kind == AerowayKind::Runway && length_m >= ASCENDING_MIN_LENGTH_M {
            let (ax, az) = axis_point(strip, strip.max_a);
            let elev = ((plane_len_blocks * ASCENDING_ELEV_FACTOR + ASCENDING_EXTRA_ELEV_M * scale)
                .round() as i32)
                .max(1);
            out.push(Placement {
                rep_id: strip.rep_id,
                kind: PlaneKind::Ascending,
                anchor_x: ax,
                anchor_z: az,
                yaw_degrees: yaw,
                pitch_degrees: ASCENDING_PITCH_DEG,
                elevation_blocks: elev,
                footprint: footprint_around(ax, az, plane_len_blocks),
            });
        }

        // Parked/taxiing plane on the centerline, sitting fully between the ends.
        if length_m >= PARKED_MIN_LENGTH_M && rng.random_bool(park_probability) {
            let half = plane_len_blocks * 0.5;
            let lo = strip.min_a + half;
            let hi = strip.max_a - half;
            if hi > lo {
                let a = lo + (hi - lo) * rng.random_range(0.0..1.0);
                let (px, pz) = axis_point(strip, a);
                out.push(Placement {
                    rep_id: strip.rep_id,
                    kind: PlaneKind::Parked,
                    anchor_x: px,
                    anchor_z: pz,
                    yaw_degrees: yaw,
                    pitch_degrees: 0.0,
                    elevation_blocks: 0,
                    footprint: footprint_around(px, pz, plane_len_blocks),
                });
            }
        }
    }

    out
}

/// World yaw (degrees) so the model's +Z nose points along `dir`. A +Z vertex maps under the
/// world yaw to (-sin β, cos β); solving for (dir.x, dir.z) gives β = atan2(-dir.x, dir.z).
fn yaw_for_dir(dir: (f64, f64)) -> f64 {
    (-dir.0).atan2(dir.1).to_degrees()
}

fn axis_point(strip: &AerowayStrip, a: f64) -> (i32, i32) {
    (
        (strip.centroid.0 + a * strip.dir.0).round() as i32,
        (strip.centroid.1 + a * strip.dir.1).round() as i32,
    )
}

fn footprint_around(x: i32, z: i32, plane_len_blocks: f64) -> Bbox {
    let r = (plane_len_blocks * 0.5).ceil() as i32 + 4;
    Bbox {
        min_x: x - r,
        min_z: z - r,
        max_x: x + r,
        max_z: z + r,
    }
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut cur = x;
        while self.parent[cur] != root {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};
    use std::collections::HashMap as StdMap;

    fn runway_tags() -> StdMap<String, String> {
        let mut t = StdMap::new();
        t.insert("aeroway".to_string(), "runway".to_string());
        t
    }

    fn mk_node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: StdMap::new(),
            x,
            z,
        }
    }

    fn runway_way(id: u64, nodes: Vec<ProcessedNode>) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: runway_tags(),
        })
    }

    fn taxiway_way(id: u64, nodes: Vec<ProcessedNode>) -> ProcessedElement {
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), "taxiway".to_string());
        ProcessedElement::Way(ProcessedWay { id, nodes, tags })
    }

    #[test]
    fn merges_collinear_split_segments() {
        // Two halves of one 200-block runway, sharing the middle node id (5).
        let a = runway_way(1, vec![mk_node(4, 0, 0), mk_node(5, 100, 0)]);
        let b = runway_way(2, vec![mk_node(5, 100, 0), mk_node(6, 200, 0)]);
        let runways = collect_aeroways(&[a, b]);
        assert_eq!(runways.len(), 1, "split runway should merge into one");
        assert!((runways[0].length_blocks - 200.0).abs() < 1e-6);
        assert_eq!(runways[0].rep_id, 1);
    }

    #[test]
    fn does_not_merge_crossing_segments() {
        // Horizontal + vertical sharing node id 5: a crossing, not a continuation.
        let a = runway_way(1, vec![mk_node(4, 0, 0), mk_node(5, 100, 0)]);
        let b = runway_way(2, vec![mk_node(5, 100, 0), mk_node(6, 100, 100)]);
        let runways = collect_aeroways(&[a, b]);
        assert_eq!(runways.len(), 2, "crossing runways must stay separate");
    }

    #[test]
    fn geometry_direction_is_long_axis() {
        // A Z-aligned runway: long axis must come back as ~(0, ±1), not the short X axis.
        let rw = runway_way(7, vec![mk_node(1, 0, -150), mk_node(2, 0, 150)]);
        let runways = collect_aeroways(&[rw]);
        assert_eq!(runways.len(), 1);
        let g = &runways[0];
        assert!((g.length_blocks - 300.0).abs() < 1e-6);
        assert!(
            g.dir.0.abs() < 1e-6 && g.dir.1.abs() > 0.99,
            "dir = {:?}",
            g.dir
        );
    }

    #[test]
    fn yaw_points_nose_along_runway() {
        // +Z direction needs zero yaw; +X needs -90°.
        assert!((yaw_for_dir((0.0, 1.0))).abs() < 1e-6);
        assert!((yaw_for_dir((1.0, 0.0)) - (-90.0)).abs() < 1e-6);
    }

    #[test]
    fn long_runway_always_gets_a_climbing_plane() {
        // 2000 blocks @ scale 1 = 2000 m >= 1500 m threshold.
        let rw = runway_way(11, vec![mk_node(1, 0, 0), mk_node(2, 2000, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, 1.0);
        assert!(placements
            .iter()
            .any(|p| p.kind == PlaneKind::Ascending && p.pitch_degrees > 0.0));
    }

    #[test]
    fn short_runway_gets_no_climbing_plane() {
        // 800 m < 1500 m.
        let rw = runway_way(13, vec![mk_node(1, 0, 0), mk_node(2, 800, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, 1.0);
        assert!(!placements.iter().any(|p| p.kind == PlaneKind::Ascending));
    }

    #[test]
    fn tiny_runway_never_parks_a_plane() {
        // 40 m < PARKED_MIN_LENGTH_M; no parked plane regardless of the RNG roll.
        let rw = runway_way(17, vec![mk_node(1, 0, 0), mk_node(2, 40, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, 1.0);
        assert!(placements.is_empty());
    }

    #[test]
    fn parked_placement_is_deterministic_and_on_runway() {
        // Find an id whose roll succeeds, on a 300 m runway (no ascending consumes RNG first).
        let id = (1u64..10_000)
            .find(|&i| element_rng(i).random_bool(RUNWAY_PARK_PROBABILITY))
            .expect("some id should roll a parked plane");
        let rw = runway_way(id, vec![mk_node(1, 0, 0), mk_node(2, 300, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, 1.0);
        let parked: Vec<_> = placements
            .iter()
            .filter(|p| p.kind == PlaneKind::Parked)
            .collect();
        assert_eq!(
            parked.len(),
            1,
            "rolled id should produce exactly one parked plane"
        );
        let half = (PLANE_LENGTH_M * 0.5) as i32;
        assert!(
            parked[0].anchor_x >= half && parked[0].anchor_x <= 300 - half,
            "parked plane must sit fully on the runway, x = {}",
            parked[0].anchor_x
        );
        assert_eq!(parked[0].pitch_degrees, 0.0);
        assert_eq!(parked[0].elevation_blocks, 0);
    }

    #[test]
    fn area_runways_are_ignored() {
        let mut tags = runway_tags();
        tags.insert("area".to_string(), "yes".to_string());
        let area = ProcessedElement::Way(ProcessedWay {
            id: 99,
            nodes: vec![
                mk_node(1, 0, 0),
                mk_node(2, 60, 0),
                mk_node(3, 60, 20),
                mk_node(4, 0, 20),
            ],
            tags,
        });
        assert!(collect_aeroways(&[area]).is_empty());
    }

    #[test]
    fn taxiway_and_runway_do_not_merge_end_on() {
        // A taxiway meeting a runway at a shared, collinear end node must stay a separate strip.
        let runway = runway_way(1, vec![mk_node(4, 0, 0), mk_node(5, 100, 0)]);
        let taxiway = taxiway_way(2, vec![mk_node(5, 100, 0), mk_node(6, 200, 0)]);
        let strips = collect_aeroways(&[runway, taxiway]);
        assert_eq!(strips.len(), 2, "different kinds must not fuse");
        assert!(strips.iter().any(|s| s.kind == AerowayKind::Runway));
        assert!(strips.iter().any(|s| s.kind == AerowayKind::Taxiway));
    }

    #[test]
    fn taxiway_never_gets_a_climbing_plane() {
        // A 2 km straight taxiway: parking is allowed, climbing is not.
        let id = (1u64..10_000)
            .find(|&i| element_rng(i).random_bool(TAXIWAY_PARK_PROBABILITY))
            .expect("some id should roll a taxiing plane");
        let tw = taxiway_way(id, vec![mk_node(1, 0, 0), mk_node(2, 2000, 0)]);
        let strips = collect_aeroways(&[tw]);
        let placements = build_placements(&strips, 1.0);
        assert!(!placements.iter().any(|p| p.kind == PlaneKind::Ascending));
        assert!(placements.iter().any(|p| p.kind == PlaneKind::Parked));
    }

    #[test]
    fn curved_taxiway_is_rejected_for_parking() {
        // A single L-shaped taxiway way is long enough but far too bent; its perpendicular
        // spread blows past the taxiway straightness cap, so no plane is parked on it.
        let bent = taxiway_way(
            1,
            vec![
                mk_node(10, 0, 0),
                mk_node(11, 200, 0),
                mk_node(12, 200, 200),
            ],
        );
        let strips = collect_aeroways(&[bent]);
        assert_eq!(strips.len(), 1);
        let perp_ratio = strips[0].perp_blocks / strips[0].length_blocks;
        assert!(
            perp_ratio > TAXIWAY_MAX_PERP_RATIO,
            "L-bend perp_ratio {perp_ratio:.3} should exceed the cap"
        );
        assert!(
            build_placements(&strips, 1.0).is_empty(),
            "a bent taxiway should not park a plane"
        );
    }
}
