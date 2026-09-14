//! Aeroplanes at aircraft stands, on runways and long straight taxiways, plus one climbing off
//! the end of a long runway. OSM splits runways into several ways with nothing linking them, so
//! same kind segments are merged by shared end node and bearing before anything is placed.
//! That merge needs the whole element list, hence a prescan plus one post merge pass.

use std::sync::OnceLock;

use crate::args::Args;
use crate::deterministic_rng::element_rng;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use rand::Rng;
use std::collections::HashMap;

use super::schematic::{place_structure_yaw, ColumnSchematic};

/// Six airline liveries, wheels down: parked and taxiing aircraft.
static GEAR_DOWN: [&[u8]; 6] = [
    include_bytes!("../../assets/structures/planes/plane_gear_down_1.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_down_2.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_down_3.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_down_4.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_down_5.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_down_6.schem"),
];
/// The same six liveries with the gear retracted, for the climbing aircraft.
static GEAR_UP: [&[u8]; 6] = [
    include_bytes!("../../assets/structures/planes/plane_gear_up_1.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_up_2.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_up_3.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_up_4.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_up_5.schem"),
    include_bytes!("../../assets/structures/planes/plane_gear_up_6.schem"),
];

/// Nose to tail length of the airframe in blocks. Nose is -Z, tail +Z, wings along X.
/// A block count, never multiplied by --scale. The metre thresholds below convert instead.
const PLANE_LENGTH_BLOCKS: f64 = 40.0;
/// Wingspan of the bundled airframe, in blocks. With the length it bounds the yawed footprint.
const PLANE_WINGSPAN_BLOCKS: f64 = 45.0;
/// Nose wheel sits this far behind the nose tip in the model, measured off the asset.
const NOSE_GEAR_OFFSET_BLOCKS: f64 = 6.0;
/// Chance a mapped stand actually holds an aircraft.
const STAND_OCCUPANCY_PROBABILITY: f64 = 0.55;
/// Closest two parked planes may sit, centre to centre. The hull is 45 blocks across.
const MIN_PLANE_SEPARATION_BLOCKS: f64 = PLANE_WINGSPAN_BLOCKS + 10.0;
/// Backstop so a huge bbox or a mis-tagged airfield cannot place an unbounded number of planes.
const MAX_PLANES: usize = 2000;
/// Half-side of the square a stamped plane can touch, at any yaw. Measured max reach is 23.
const PLANE_REACH_BLOCKS: i32 = 27;
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
/// Per-taxiway chance of a taxiing plane -- lower than runways, which are far fewer.
const TAXIWAY_PARK_PROBABILITY: f64 = 0.15;
/// A parked plane needs a strip at least this long so it sits fully between the ends.
const PARKED_MIN_LENGTH_M: f64 = 120.0;
/// Above this length an aeroway is almost certainly mis-tagged (a whole airfield); skip it.
const MAX_AEROWAY_LENGTH_M: f64 = 8000.0;
/// Two segments sharing an end node merge only if their bearings differ by less than this.
const COLLINEAR_TOL_RAD: f64 = 0.349; // ~20 deg
/// Straightness cap (perpendicular / long extent); stricter for curvy taxiways than runways.
const RUNWAY_MAX_PERP_RATIO: f64 = 0.5;
const TAXIWAY_MAX_PERP_RATIO: f64 = 0.12;

/// Parsed liveries, keyed by gear state. Loaded once; a broken asset is dropped with a warning.
fn fleet(gear_down: bool) -> &'static [ColumnSchematic] {
    static DOWN: OnceLock<Vec<ColumnSchematic>> = OnceLock::new();
    static UP: OnceLock<Vec<ColumnSchematic>> = OnceLock::new();
    let load = |src: &[&'static [u8]]| -> Vec<ColumnSchematic> {
        src.iter()
            .filter_map(|bytes| match ColumnSchematic::load(bytes) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!(
                        "{} plane model failed to load: {e}",
                        "Warning:".yellow().bold()
                    );
                    None
                }
            })
            .collect()
    };
    if gear_down {
        DOWN.get_or_init(|| load(&GEAR_DOWN))
    } else {
        UP.get_or_init(|| load(&GEAR_UP))
    }
}

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
    /// World yaw (degrees) that points the model's -Z nose along the aeroway direction.
    yaw_degrees: f64,
    pitch_degrees: f64,
    elevation_blocks: i32,
    footprint: Bbox,
}

pub struct PrescanResult {
    placements: Vec<Placement>,
}

impl PrescanResult {
    /// Exactly the regions the stamps touch. The shared region_keys_around pads by a whole
    /// region each side, which for a 27 block model pins about four times more than needed.
    pub fn deferred_region_keys(&self) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for p in &self.placements {
            let (rx0, rx1) = (
                (p.anchor_x - PLANE_REACH_BLOCKS) >> 9,
                (p.anchor_x + PLANE_REACH_BLOCKS) >> 9,
            );
            let (rz0, rz1) = (
                (p.anchor_z - PLANE_REACH_BLOCKS) >> 9,
                (p.anchor_z + PLANE_REACH_BLOCKS) >> 9,
            );
            for rx in rx0..=rx1 {
                for rz in rz0..=rz1 {
                    out.push((rx, rz));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Decides where planes go, before the tile loop. Returns nothing when props are off so the
/// caller does not pin regions for placements that will never be stamped.
pub fn prescan(elements: &[ProcessedElement], args: &Args) -> PrescanResult {
    if !args.use_3d {
        return PrescanResult {
            placements: Vec::new(),
        };
    }
    PrescanResult {
        placements: build_placements(
            &collect_aeroways(elements),
            &collect_stands(elements),
            args.scale,
        ),
    }
}

/// Stamp the planes, after ground generation so the runway Y is final.
pub fn place_plane_models(editor: &mut WorldEditor, prescan: &PrescanResult) {
    if prescan.placements.is_empty() || !editor.place_schematics() {
        return;
    }

    // Resolve the assets before announcing, so a load failure cannot promise planes.
    if fleet(true).is_empty() && fleet(false).is_empty() {
        return;
    }
    println!(
        "{} Placing {} plane{}...",
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

    for p in &prescan.placements {
        let fleet = fleet(p.kind == PlaneKind::Parked);
        if fleet.is_empty() {
            continue;
        }
        // Livery is drawn from the strip's own id, so a tile boundary cannot change it.
        let livery = (element_rng(p.rep_id.wrapping_mul(31).wrapping_add(7)).random::<u32>()
            as usize)
            % fleet.len();

        let fp = &p.footprint;
        let ground_y =
            crate::models_3d::lowest_ground_in_bbox(editor, fp.min_x, fp.min_z, fp.max_x, fp.max_z);

        place_structure_yaw(
            editor,
            &fleet[livery],
            p.anchor_x,
            p.anchor_z,
            // +1 so the wheels rest ON the runway surface, which itself sits at ground level.
            ground_y + 1 + p.elevation_blocks,
            p.yaw_degrees,
            p.pitch_degrees,
        );
        match p.kind {
            PlaneKind::Parked => parked += 1,
            PlaneKind::Ascending => climbing += 1,
        }
    }

    println!(
        "  Placed {} plane{} ({parked} parked, {climbing} climbing)",
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

/// One `aeroway=parking_position` way: an aircraft stand with a stated heading.
#[derive(Clone, Copy, Debug)]
struct Stand {
    way_id: u64,
    /// Last node of the way, which OSM defines as the nose wheel position.
    nose_x: i32,
    nose_z: i32,
    /// Unit vector of the last segment, the direction the aircraft faces.
    dir: (f64, f64),
}

/// Stands are drawn taxiway to nose wheel, so the way says where a plane parks and which way.
fn collect_stands(elements: &[ProcessedElement]) -> Vec<Stand> {
    let mut out = Vec::new();
    for element in elements {
        let ProcessedElement::Way(w) = element else {
            continue;
        };
        if w.tags.get("aeroway").map(String::as_str) != Some("parking_position") {
            continue;
        }
        if w.tags.get("area").is_some_and(|v| v == "yes") || w.nodes.len() < 2 {
            continue;
        }
        let (a, b) = (&w.nodes[w.nodes.len() - 2], &w.nodes[w.nodes.len() - 1]);
        if a.id == b.id {
            continue;
        }
        let (dx, dz) = (f64::from(b.x - a.x), f64::from(b.z - a.z));
        let len = dx.hypot(dz);
        if len < 1.0 {
            continue;
        }
        out.push(Stand {
            way_id: w.id,
            nose_x: b.x,
            nose_z: b.z,
            dir: (dx / len, dz / len),
        });
    }
    out
}

/// Drops any placement crowding one already kept. The hash grid keeps this linear, so thousands
/// of stands cost no more per plane than one airport does.
fn thin_by_spacing(placements: Vec<Placement>) -> Vec<Placement> {
    let cell = MIN_PLANE_SEPARATION_BLOCKS.ceil() as i32;
    let min_sq = MIN_PLANE_SEPARATION_BLOCKS * MIN_PLANE_SEPARATION_BLOCKS;
    let mut grid: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    let mut kept = Vec::with_capacity(placements.len());
    for p in placements {
        // Climbing planes are tens of blocks up, so they cannot hit anything on the ground.
        if p.kind == PlaneKind::Ascending {
            kept.push(p);
            continue;
        }
        let (cx, cz) = (p.anchor_x.div_euclid(cell), p.anchor_z.div_euclid(cell));
        let crowded = (-1..=1).any(|gx| {
            (-1..=1).any(|gz| {
                grid.get(&(cx + gx, cz + gz)).is_some_and(|bucket| {
                    bucket.iter().any(|&(qx, qz)| {
                        let (dx, dz) = (f64::from(p.anchor_x - qx), f64::from(p.anchor_z - qz));
                        dx * dx + dz * dz < min_sq
                    })
                })
            })
        });
        if crowded {
            continue;
        }
        grid.entry((cx, cz))
            .or_default()
            .push((p.anchor_x, p.anchor_z));
        kept.push(p);
    }
    kept
}

// ---------------------------------------------------------------------------
// Placement decisions
// ---------------------------------------------------------------------------

fn build_placements(strips: &[AerowayStrip], stands: &[Stand], scale: f64) -> Vec<Placement> {
    let mut out = Vec::new();

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
            // Proportional part is a fraction of the model, the flat part is metres.
            let elev = ((PLANE_LENGTH_BLOCKS * ASCENDING_ELEV_FACTOR
                + ASCENDING_EXTRA_ELEV_M * scale)
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
                footprint: footprint_around(ax, az),
            });
        }

        // Parked/taxiing plane on the centerline, sitting fully between the ends.
        if length_m >= PARKED_MIN_LENGTH_M && rng.random_bool(park_probability) {
            // The model is a fixed 40 blocks, so the fit test is in blocks, not scaled metres.
            let half = PLANE_LENGTH_BLOCKS * 0.5;
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
                    footprint: footprint_around(px, pz),
                });
            }
        }
    }

    // Stands last so a runway or taxiway plane, which needs a specific spot, wins any crowding.
    for stand in stands {
        let mut rng = element_rng(stand.way_id);
        if !rng.random_bool(STAND_OCCUPANCY_PROBABILITY) {
            continue;
        }
        // Anchor is the model centre; back it off so the nose wheel lands on the mapped node.
        let back = PLANE_LENGTH_BLOCKS * 0.5 - NOSE_GEAR_OFFSET_BLOCKS;
        let ax = (f64::from(stand.nose_x) - stand.dir.0 * back).round() as i32;
        let az = (f64::from(stand.nose_z) - stand.dir.1 * back).round() as i32;
        out.push(Placement {
            rep_id: stand.way_id,
            kind: PlaneKind::Parked,
            anchor_x: ax,
            anchor_z: az,
            yaw_degrees: yaw_for_dir(stand.dir),
            pitch_degrees: 0.0,
            elevation_blocks: 0,
            footprint: footprint_around(ax, az),
        });
    }

    let mut out = thin_by_spacing(out);
    if out.len() > MAX_PLANES {
        eprintln!(
            "{} {} plane spots found, placing the first {MAX_PLANES}",
            "Warning:".yellow().bold(),
            out.len()
        );
        out.truncate(MAX_PLANES);
    }
    out
}

/// World yaw pointing the model's -Z nose along `dir`.
fn yaw_for_dir(dir: (f64, f64)) -> f64 {
    dir.0.atan2(-dir.1).to_degrees()
}

fn axis_point(strip: &AerowayStrip, a: f64) -> (i32, i32) {
    (
        (strip.centroid.0 + a * strip.dir.0).round() as i32,
        (strip.centroid.1 + a * strip.dir.1).round() as i32,
    )
}

/// Square the stamp can touch at any yaw, used to sample the ground under it. Kept tight because
/// lowest_ground_in_bbox takes the minimum, so a wider probe only sinks the plane into a dip.
fn footprint_around(x: i32, z: i32) -> Bbox {
    let r = PLANE_REACH_BLOCKS;
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

    /// A bad split shows up as a size drift, not a parse failure.
    #[test]
    fn parked_fit_uses_the_models_real_block_length_at_any_scale() {
        let id = (1u64..10_000)
            .find(|&i| element_rng(i).random_bool(RUNWAY_PARK_PROBABILITY))
            .expect("some id should roll a parked plane");
        // 120 blocks of runway is three model lengths, so the fit test is what matters here.
        let rw = runway_way(id, vec![mk_node(1, 0, 0), mk_node(2, 120, 0)]);
        let strips = collect_aeroways(&rw_slice(rw));
        for p in build_placements(&strips, &[], 0.3) {
            if p.kind != PlaneKind::Parked {
                continue;
            }
            let half = (PLANE_LENGTH_BLOCKS * 0.5) as i32;
            assert!(
                p.anchor_x >= half && p.anchor_x <= 120 - half,
                "parked plane hangs off the runway at scale 0.3, x = {}",
                p.anchor_x
            );
        }
    }

    fn stand_way(id: u64, nodes: Vec<ProcessedNode>) -> ProcessedElement {
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), "parking_position".to_string());
        ProcessedElement::Way(ProcessedWay { id, nodes, tags })
    }

    /// Nose gear lands on the last node, facing the way the stand was drawn.
    #[test]
    fn stand_puts_the_nose_gear_on_the_last_node() {
        let stands = collect_stands(&[stand_way(5, vec![mk_node(1, 0, 0), mk_node(2, 0, 100)])]);
        assert_eq!(stands.len(), 1);
        let s = stands[0];
        assert_eq!((s.nose_x, s.nose_z), (0, 100));
        assert!((s.dir.1 - 1.0).abs() < 1e-9);
        // Nose points +Z, so the anchor sits back along -Z by half a hull less the gear offset.
        let back = PLANE_LENGTH_BLOCKS * 0.5 - NOSE_GEAR_OFFSET_BLOCKS;
        let placed = build_placements(&[], &stands, 1.0);
        if let Some(p) = placed.first() {
            assert_eq!(p.anchor_z, 100 - back as i32);
            assert_eq!(p.anchor_x, 0);
            assert!((p.yaw_degrees.abs() - 180.0).abs() < 1e-6);
        }
    }

    #[test]
    fn stands_reject_areas_and_degenerate_ways() {
        let mut area = stand_way(1, vec![mk_node(1, 0, 0), mk_node(2, 0, 60)]);
        if let ProcessedElement::Way(w) = &mut area {
            w.tags.insert("area".to_string(), "yes".to_string());
        }
        assert!(collect_stands(&[area]).is_empty());
        assert!(collect_stands(&[stand_way(2, vec![mk_node(1, 7, 7)])]).is_empty());
        // Zero length last segment carries no heading.
        assert!(
            collect_stands(&[stand_way(3, vec![mk_node(1, 0, 0), mk_node(2, 0, 0)])]).is_empty()
        );
    }

    /// A row of stands packed tighter than a wingspan must come out thinned, not overlapping.
    #[test]
    fn crowded_stands_are_thinned_to_the_separation() {
        let ways: Vec<ProcessedElement> = (0..40)
            .map(|i| {
                stand_way(
                    1000 + i as u64,
                    vec![mk_node(1, i * 12, 0), mk_node(2, i * 12, 60)],
                )
            })
            .collect();
        let placed = build_placements(&[], &collect_stands(&ways), 1.0);
        assert!(placed.len() > 1, "some stands should survive");
        for (i, a) in placed.iter().enumerate() {
            for b in &placed[i + 1..] {
                let (dx, dz) = (
                    f64::from(a.anchor_x - b.anchor_x),
                    f64::from(a.anchor_z - b.anchor_z),
                );
                assert!(
                    dx.hypot(dz) >= MIN_PLANE_SEPARATION_BLOCKS,
                    "planes {} and {} are {:.1} blocks apart",
                    a.rep_id,
                    b.rep_id,
                    dx.hypot(dz)
                );
            }
        }
    }

    /// A pathological airfield must not be able to place an unbounded number of planes.
    #[test]
    fn placement_count_is_capped() {
        let ways: Vec<ProcessedElement> = (0..MAX_PLANES as i32 + 500)
            .map(|i| {
                stand_way(
                    9_000_000 + i as u64,
                    vec![
                        mk_node(1, (i % 200) * 60, (i / 200) * 60),
                        mk_node(2, (i % 200) * 60, (i / 200) * 60 + 50),
                    ],
                )
            })
            .collect();
        let placed = build_placements(&[], &collect_stands(&ways), 1.0);
        assert!(placed.len() <= MAX_PLANES, "got {}", placed.len());
    }

    #[test]
    fn all_plane_assets_parse_at_the_expected_size() {
        for gear_down in [true, false] {
            let fleet = fleet(gear_down);
            assert_eq!(fleet.len(), 6, "gear_down={gear_down}");
            for plane in fleet {
                assert_eq!((plane.width, plane.length), (45, 40));
                assert_eq!(plane.height(), if gear_down { 13 } else { 12 });
            }
        }
    }

    /// A shear, not a resample: the nose end lifts, the tail end drops, and the centre holds.
    #[test]
    fn pitch_lifts_the_nose_and_drops_the_tail() {
        use crate::structures::schematic::pitch_lift;
        let (nose, mid, tail) = (
            pitch_lift(40, 0, ASCENDING_PITCH_DEG),
            pitch_lift(40, 20, ASCENDING_PITCH_DEG),
            pitch_lift(40, 39, ASCENDING_PITCH_DEG),
        );
        assert!(nose > 0 && tail < 0, "nose {nose}, tail {tail}");
        assert!(mid.abs() <= 1, "the centre slice barely moves, got {mid}");
        assert_eq!(nose, -tail, "the shear must be symmetric about the centre");
        assert_eq!(pitch_lift(40, 0, 0.0), 0, "no pitch, no lift");
    }

    fn rw_slice(w: ProcessedElement) -> [ProcessedElement; 1] {
        [w]
    }

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
        // The airframe's nose is -Z, so a +Z runway needs a half turn and +X needs +90°.
        assert!((yaw_for_dir((0.0, 1.0)).abs() - 180.0).abs() < 1e-6);
        assert!((yaw_for_dir((1.0, 0.0)) - 90.0).abs() < 1e-6);
        assert!((yaw_for_dir((0.0, -1.0))).abs() < 1e-6);
    }

    #[test]
    fn long_runway_always_gets_a_climbing_plane() {
        // 2000 blocks @ scale 1 = 2000 m >= 1500 m threshold.
        let rw = runway_way(11, vec![mk_node(1, 0, 0), mk_node(2, 2000, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, &[], 1.0);
        assert!(placements
            .iter()
            .any(|p| p.kind == PlaneKind::Ascending && p.pitch_degrees > 0.0));
    }

    #[test]
    fn short_runway_gets_no_climbing_plane() {
        // 800 m < 1500 m.
        let rw = runway_way(13, vec![mk_node(1, 0, 0), mk_node(2, 800, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, &[], 1.0);
        assert!(!placements.iter().any(|p| p.kind == PlaneKind::Ascending));
    }

    #[test]
    fn tiny_runway_never_parks_a_plane() {
        // 40 m < PARKED_MIN_LENGTH_M; no parked plane regardless of the RNG roll.
        let rw = runway_way(17, vec![mk_node(1, 0, 0), mk_node(2, 40, 0)]);
        let runways = collect_aeroways(&[rw]);
        let placements = build_placements(&runways, &[], 1.0);
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
        let placements = build_placements(&runways, &[], 1.0);
        let parked: Vec<_> = placements
            .iter()
            .filter(|p| p.kind == PlaneKind::Parked)
            .collect();
        assert_eq!(
            parked.len(),
            1,
            "rolled id should produce exactly one parked plane"
        );
        let half = (PLANE_LENGTH_BLOCKS * 0.5) as i32;
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
        let placements = build_placements(&strips, &[], 1.0);
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
            build_placements(&strips, &[], 1.0).is_empty(),
            "a bent taxiway should not park a plane"
        );
    }
}
