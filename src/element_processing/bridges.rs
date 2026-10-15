use crate::bresenham::bresenham_line;
use crate::element_processing::bridge_modules;
use crate::element_processing::bridge_styles::{
    default_pylons, resolve_bridge_style_with_outline, BridgeOutlineIndex, BridgeStyle,
};
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::railways;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};

const LAYER_HEIGHT_STEP: i32 = 6;
const FLAT_TERRAIN_DIP_THRESHOLD: i32 = 4;
const SHORT_BRIDGE_LENGTH_BLOCKS: usize = 30;
const BRIDGE_NAME_FUSE_DISTANCE_BLOCKS: i32 = 200;
const DUAL_CARRIAGEWAY_MAX_DISTANCE_BLOCKS: f32 = 12.0;
const DUAL_CARRIAGEWAY_HEADING_TOLERANCE_DEG: f32 = 20.0;
/// Parallel decks whose edges come this close read as one structure, whatever their layers.
const SIDE_DECK_EDGE_GAP_BLOCKS: f32 = 2.0;
/// Deck Y above the ground where a road, path or track passes underneath.
const ROAD_HEADROOM: i32 = 6;
const PATH_HEADROOM: i32 = 5;
// Clears the catenary wire, 6 above the track bed.
const RAIL_HEADROOM: i32 = 8;
/// Deck Y above a river's, canal's or stream's centerline.
const RIVER_HEADROOM: i32 = 4;
const STREAM_HEADROOM: i32 = 2;
/// Deck Y above a lower bridge deck this one crosses.
const STACKED_DECK_HEADROOM: i32 = 6;
/// Room an arch needs over flat ground for its curve to read.
const ARCH_MIN_CLEARANCE: i32 = 8;
/// Steepest climb (blocks per cell) a clearance may force from a deck end.
const MAX_CLEARANCE_SLOPE: f32 = 1.0;
/// How far a member may double back along its group's axis and still share one profile.
const PROFILE_MONOTONIC_TOLERANCE: f32 = 2.0;
/// Bucket side of the spatial index over at-grade ways.
const OBSTACLE_GRID_CELL: i32 = 64;
/// A deck at least this far below another in the same column is a separate, lower level.
const LOWER_DECK_GAP: i32 = STACKED_DECK_HEADROOM - 1;
/// Rows a deck's own structure reaches below its surface.
const DECK_STRUCTURE_DEPTH: i32 = 2;
/// How far off a road deck a rail bridge's track may run and still ride on it.
const RAIL_ON_DECK_REACH: i32 = 2;

fn is_non_vehicular_bridge_highway(highway: &str) -> bool {
    matches!(
        highway,
        "footway" | "pedestrian" | "path" | "cycleway" | "bridleway" | "steps"
    )
}

#[derive(Clone)]
pub struct BridgeMemberInfo {
    // Deck Y per centerline cell, indexed like the renderer's cumulative bresenham walk.
    ys: Vec<i32>,
    pub style: BridgeStyle,
    // True when the endpoint is not shared with another member of the group.
    pub start_is_group_boundary: bool,
    pub end_is_group_boundary: bool,
    // Deck module this member sweeps; None when covered, pedestrian or procedural.
    pub module_idx: Option<usize>,
    // True when a wider parallel member's module deck covers this way; skip it.
    pub covered_by_wider: bool,
    // Cable spans: pylon positions from the main span; None stands on piers.
    pub cable_pylons: Option<Vec<(i32, i32)>>,
}

impl BridgeMemberInfo {
    /// Deck Y at centerline cell `tds`; cells past the end keep the last value.
    pub fn y_at(&self, tds: usize) -> i32 {
        self.ys
            .get(tds)
            .or(self.ys.last())
            .copied()
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy)]
pub struct BridgeRampInfo {
    // True if way.nodes[0] is the bridge-side end; false if way.nodes[len-1] is.
    pub bridge_side_at_start: bool,
    pub deck_y: i32,
    pub ground_y: i32,
}

impl BridgeRampInfo {
    pub fn y_at(&self, tds: usize, total_bresenham: usize) -> i32 {
        if total_bresenham == 0 {
            return self.deck_y;
        }
        let last_idx = total_bresenham - 1;
        let denom = last_idx.max(1) as f32;
        let (start_y, end_y) = if self.bridge_side_at_start {
            (self.deck_y, self.ground_y)
        } else {
            (self.ground_y, self.deck_y)
        };
        let t = (tds as f32 / denom).min(1.0);
        let span = (end_y - start_y) as f32;
        (start_y as f32 + span * t).round() as i32
    }
}

/// How a rail bridge way is carried.
pub enum RailDeck {
    /// Its own deck at one Y, shared by every way of the viaduct.
    Level(i32),
    /// Rides a road deck, at its Y per smoothed centerline cell.
    Carried(Vec<i32>),
}

pub struct BridgeStructureMap {
    members: HashMap<u64, BridgeMemberInfo>,
    ramps: HashMap<u64, BridgeRampInfo>,
    rail_decks: HashMap<u64, RailDeck>,
}

impl BridgeStructureMap {
    pub fn lookup_member(&self, way_id: u64) -> Option<&BridgeMemberInfo> {
        self.members.get(&way_id)
    }

    pub fn lookup_ramp(&self, way_id: u64) -> Option<&BridgeRampInfo> {
        self.ramps.get(&way_id)
    }

    /// Deck of the rail bridge `way_id`.
    pub fn rail_deck(&self, way_id: u64) -> Option<&RailDeck> {
        self.rail_decks.get(&way_id)
    }

    pub fn build(
        elements: &[ProcessedElement],
        editor: &WorldEditor,
        outlines: &BridgeOutlineIndex,
        scale: f64,
    ) -> Self {
        let mut bridge_ways: Vec<&ProcessedWay> = Vec::new();
        let mut other_highway_ways: Vec<&ProcessedWay> = Vec::new();
        let mut rail_bridge_ways: Vec<&ProcessedWay> = Vec::new();
        for elem in elements {
            if let ProcessedElement::Way(w) = elem {
                if railways::renders_as_rail_bridge(w) {
                    rail_bridge_ways.push(w);
                }
                if !w.tags.contains_key("highway") || w.nodes.len() < 2 {
                    continue;
                }
                if is_bridge_way(w) {
                    bridge_ways.push(w);
                } else {
                    other_highway_ways.push(w);
                }
            }
        }

        let mut members: HashMap<u64, BridgeMemberInfo> = HashMap::new();
        let mut ramps: HashMap<u64, BridgeRampInfo> = HashMap::new();
        let mut rail_decks: HashMap<u64, RailDeck> = HashMap::new();
        if bridge_ways.is_empty() && rail_bridge_ways.is_empty() {
            return Self {
                members,
                ramps,
                rail_decks,
            };
        }

        let mut node_to_bridge_indices: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, way) in bridge_ways.iter().enumerate() {
            let start = &way.nodes[0];
            let end = &way.nodes[way.nodes.len() - 1];
            node_to_bridge_indices
                .entry((start.x, start.z))
                .or_default()
                .push(i);
            if (end.x, end.z) != (start.x, start.z) {
                node_to_bridge_indices
                    .entry((end.x, end.z))
                    .or_default()
                    .push(i);
            }
        }

        let mut uf = UnionFind::new(bridge_ways.len());

        // Step 1: union by shared endpoint at the same effective layer.
        for indices in node_to_bridge_indices.values() {
            if indices.len() < 2 {
                continue;
            }
            let mut by_layer: HashMap<i32, Vec<usize>> = HashMap::new();
            for &idx in indices {
                let layer = effective_layer(bridge_ways[idx]);
                by_layer.entry(layer).or_default().push(idx);
            }
            for group in by_layer.values() {
                if group.len() < 2 {
                    continue;
                }
                let first = group[0];
                for &other in &group[1..] {
                    uf.union(first, other);
                }
            }
        }

        // Step 2: union by shared bridge:name with close centroids.
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, way) in bridge_ways.iter().enumerate() {
            if let Some(name) = way.tags.get("bridge:name") {
                if !name.is_empty() {
                    by_name.entry(name.as_str()).or_default().push(i);
                }
            }
        }
        for group in by_name.values() {
            if group.len() < 2 {
                continue;
            }
            let centroids: Vec<(i32, i32)> =
                group.iter().map(|&i| centroid(bridge_ways[i])).collect();
            for i in 0..group.len() {
                for j in (i + 1)..group.len() {
                    let a = group[i];
                    let b = group[j];
                    if effective_layer(bridge_ways[a]) != effective_layer(bridge_ways[b]) {
                        continue;
                    }
                    let dx = (centroids[i].0 - centroids[j].0).abs();
                    let dz = (centroids[i].1 - centroids[j].1).abs();
                    if dx <= BRIDGE_NAME_FUSE_DISTANCE_BLOCKS
                        && dz <= BRIDGE_NAME_FUSE_DISTANCE_BLOCKS
                    {
                        uf.union(a, b);
                    }
                }
            }
        }

        // Step 3: union decks running side by side (dual carriageways, sidewalks).
        let bboxes: Vec<(i32, i32, i32, i32)> = bridge_ways.iter().map(|w| way_bbox(w)).collect();
        let reach = |w: &ProcessedWay| way_half_width(w, scale) as f32 + 1.0;
        for a in 0..bridge_ways.len() {
            for b in (a + 1)..bridge_ways.len() {
                if uf.find(a) == uf.find(b) {
                    continue;
                }
                let margin = (reach(bridge_ways[a]) + reach(bridge_ways[b]))
                    .max(DUAL_CARRIAGEWAY_MAX_DISTANCE_BLOCKS)
                    .ceil() as i32
                    + 2;
                if !bboxes_touch(bboxes[a], bboxes[b], margin) {
                    continue;
                }
                if side_by_side(bridge_ways[a], bridge_ways[b], scale) {
                    uf.union(a, b);
                }
            }
        }

        // Group bridge ways by root; lower structures first so decks above can clear them.
        let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..bridge_ways.len() {
            by_root.entry(uf.find(i)).or_default().push(i);
        }
        let mut groups: Vec<Vec<usize>> = by_root.into_values().collect();
        let group_layer = |g: &Vec<usize>| {
            g.iter()
                .map(|&i| effective_layer(bridge_ways[i]))
                .max()
                .unwrap_or(0)
        };
        groups.sort_by_key(|g| (group_layer(g), g[0]));

        // Build node -> non-bridge highway ways index for ramp detection.
        let mut node_to_other_highways: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, way) in other_highway_ways.iter().enumerate() {
            let start = &way.nodes[0];
            let end = &way.nodes[way.nodes.len() - 1];
            node_to_other_highways
                .entry((start.x, start.z))
                .or_default()
                .push(i);
            if (end.x, end.z) != (start.x, start.z) {
                node_to_other_highways
                    .entry((end.x, end.z))
                    .or_default()
                    .push(i);
            }
        }

        let bounds = bridge_ways
            .iter()
            .chain(&rail_bridge_ways)
            .map(|w| way_bbox(w))
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
            .unwrap_or_default();
        let obstacles = GradeObstacles::build(elements, scale, bounds);

        // Track ramp ways to make sure each ramp attaches to only one structure.
        let mut claimed_ramp_ways: HashSet<u64> = HashSet::new();
        // Deck cells of structures already resolved: (deck Y, layer), for decks crossing above.
        let mut resolved_decks: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
        // Deck Y at the member ends of structures already resolved, for joints to meet.
        let mut resolved_joints: HashMap<(i32, i32), i32> = HashMap::new();

        for group_indices in &groups {
            // Members per node; an end no other member touches is a boundary.
            let mut node_members: HashMap<(i32, i32), usize> = HashMap::new();
            let mut group_nodes: HashSet<(i32, i32)> = HashSet::new();
            for &idx in group_indices {
                let mut seen: HashSet<(i32, i32)> = HashSet::new();
                for n in &bridge_ways[idx].nodes {
                    if seen.insert((n.x, n.z)) {
                        *node_members.entry((n.x, n.z)).or_default() += 1;
                    }
                }
                group_nodes.extend(seen);
            }
            let is_boundary = |xz: (i32, i32)| node_members.get(&xz).copied().unwrap_or(0) <= 1;
            // An end shared with another structure is a joint in the air.
            let joins_other_bridge = |xz: (i32, i32)| {
                node_to_bridge_indices
                    .get(&xz)
                    .is_some_and(|ids| ids.iter().any(|i| !group_indices.contains(i)))
            };
            let ends = |idx: usize| {
                let w = bridge_ways[idx];
                let (s, e) = (&w.nodes[0], &w.nodes[w.nodes.len() - 1]);
                ((s.x, s.z), (e.x, e.z))
            };

            // Max explicit layer; unlabelled members contribute 0 so a pure-unlabelled group stays at terrain.
            let mut max_layer = 0;
            for &idx in group_indices {
                let way = bridge_ways[idx];
                if let Some(l) = way.tags.get("layer").and_then(|v| v.parse::<i32>().ok()) {
                    max_layer = max_layer.max(l.max(0));
                }
            }

            // Terrain under every centerline cell, not just the nodes.
            let cells: Vec<Vec<(i32, i32)>> = group_indices
                .iter()
                .map(|&i| way_cells(bridge_ways[i]))
                .collect();
            let terrain: Vec<Vec<i32>> = cells
                .iter()
                .map(|cs| {
                    cs.iter()
                        .map(|&(x, z)| editor.get_ground_level(x, z))
                        .collect()
                })
                .collect();
            let Some(terrain_max) = terrain.iter().flatten().copied().max() else {
                continue;
            };
            let terrain_min = terrain
                .iter()
                .flatten()
                .copied()
                .min()
                .unwrap_or(terrain_max);
            let dip = terrain_max - terrain_min;
            let total_length: usize = group_indices
                .iter()
                .map(|&i| way_length_blocks(bridge_ways[i]))
                .sum();
            // Style first so arches can force flat-terrain clearance for the curve to show.
            let group_style = majority_style(group_indices, &bridge_ways, outlines);

            // One module deck per structure: the widest member sweeps it.
            let member_range = |idx: usize| {
                let w = bridge_ways[idx];
                highway_block_range(
                    w.tags.get("highway").map(String::as_str).unwrap_or(""),
                    &w.tags,
                    1.0,
                )
            };
            let group_has_vehicular_member = group_indices.iter().any(|&i| {
                let way = bridge_ways[i];
                let highway = way.tags.get("highway").map(String::as_str).unwrap_or("");
                !is_non_vehicular_bridge_highway(highway)
            });

            let structure_has_module = group_style == BridgeStyle::Beam
                && group_has_vehicular_member
                && group_indices.iter().any(|&i| {
                    bridge_modules::pick_module_index(member_range(i), total_length).is_some()
                });

            // Redundant when fully inside a wider parallel member's module deck.
            let covers = |j: usize, idx: usize, covered_ids: &HashSet<u64>| -> bool {
                let way = bridge_ways[idx];
                let other = bridge_ways[j];
                if other.id == way.id || covered_ids.contains(&other.id) {
                    return false;
                }
                let (rj, ri) = (member_range(j), member_range(idx));
                let (lj, li) = (way_length_blocks(other), way_length_blocks(way));
                let wider = rj > ri || (rj == ri && (lj > li || (lj == li && other.id < way.id)));
                if !wider || li > lj + 30 {
                    return false;
                }
                let parallel = match (heading_deg(way), heading_deg(other)) {
                    (Some(ha), Some(hb)) => headings_parallel(ha, hb),
                    _ => false,
                };
                if !parallel {
                    return false;
                }
                let deck_half = bridge_modules::pick_module_index(rj, total_length)
                    .and_then(bridge_modules::module_half_width)
                    .unwrap_or(0);
                let mut threshold = (deck_half - 2).max(0) as f32;
                // Equal decks are only redundant at dual-carriageway distance.
                if rj == ri {
                    threshold = threshold.min(DUAL_CARRIAGEWAY_MAX_DISTANCE_BLOCKS);
                }
                // Every sample must sit inside the deck so spurs and overhangs still render.
                coverage_samples(way)
                    .iter()
                    .all(|&(px, pz)| lateral_offset_to_way(px, pz, other) <= threshold)
            };
            let mut covered_ids: HashSet<u64> = HashSet::new();
            // Survivors that absorbed an equal-width sibling sweep one size wider.
            let mut widened_ids: HashSet<u64> = HashSet::new();
            if structure_has_module && group_indices.len() > 1 {
                // Widest-first pass so covered members never cover others.
                let mut order: Vec<usize> = group_indices.clone();
                order.sort_by(|&a, &b| {
                    member_range(b)
                        .cmp(&member_range(a))
                        .then(
                            way_length_blocks(bridge_ways[b])
                                .cmp(&way_length_blocks(bridge_ways[a])),
                        )
                        .then(bridge_ways[a].id.cmp(&bridge_ways[b].id))
                });
                for &idx in &order {
                    let mut is_covered = false;
                    for &j in &order {
                        if covers(j, idx, &covered_ids) {
                            is_covered = true;
                            if member_range(j) == member_range(idx) {
                                widened_ids.insert(bridge_ways[j].id);
                            }
                        }
                    }
                    if is_covered {
                        covered_ids.insert(bridge_ways[idx].id);
                    }
                }
            }
            let is_covered = |idx: usize| covered_ids.contains(&bridge_ways[idx].id);

            // Boundary endpoints of covered ways render nothing; no ramps there.
            let mut covered_boundaries: HashSet<(i32, i32)> = HashSet::new();
            for &idx in group_indices {
                if !is_covered(idx) {
                    continue;
                }
                let (s, e) = ends(idx);
                for xz in [s, e] {
                    if is_boundary(xz) {
                        covered_boundaries.insert(xz);
                    }
                }
            }

            // Tagged ramps carry the deck down; their start Y is filled in once known.
            let mut external_ramp_ends: HashSet<(i32, i32)> = HashSet::new();
            let mut pending_ramps: Vec<(u64, bool, i32, (i32, i32))> = Vec::new();
            for &idx in group_indices {
                let (s, e) = ends(idx);
                for xz in [s, e] {
                    if !is_boundary(xz) || covered_boundaries.contains(&xz) {
                        continue;
                    }
                    let Some(other_indices) = node_to_other_highways.get(&xz) else {
                        continue;
                    };
                    for &oi in other_indices {
                        let candidate = other_highway_ways[oi];
                        if !is_ramp_candidate(candidate) {
                            continue;
                        }
                        // Each ramp can only attach once; if already claimed, skip.
                        if claimed_ramp_ways.contains(&candidate.id) {
                            continue;
                        }
                        let bridge_side_at_start =
                            (candidate.nodes[0].x, candidate.nodes[0].z) == xz;
                        let far_node = if bridge_side_at_start {
                            &candidate.nodes[candidate.nodes.len() - 1]
                        } else {
                            &candidate.nodes[0]
                        };
                        // Reject if the far end is also on this structure (connector between two decks).
                        if node_members.contains_key(&(far_node.x, far_node.z)) {
                            continue;
                        }
                        let ground_y = editor.get_ground_level(far_node.x, far_node.z);
                        pending_ramps.push((candidate.id, bridge_side_at_start, ground_y, xz));
                        claimed_ramp_ways.insert(candidate.id);
                        external_ramp_ends.insert(xz);
                    }
                }
            }
            // Where the deck has to come down to the terrain itself.
            let anchors_end = |xz: (i32, i32)| {
                is_boundary(xz)
                    && !covered_boundaries.contains(&xz)
                    && !external_ramp_ends.contains(&xz)
                    && !joins_other_bridge(xz)
            };

            // Profile axis: the two member ends furthest apart.
            let end_points: Vec<(i32, i32)> = group_indices
                .iter()
                .filter(|&&i| !is_covered(i))
                .flat_map(|&i| {
                    let (s, e) = ends(i);
                    [s, e]
                })
                .collect();
            let Some(&first_end) = end_points.first() else {
                continue;
            };
            let mut axis_ends = (first_end, first_end);
            let mut best = 0i64;
            for (i, a) in end_points.iter().enumerate() {
                for b in &end_points[i + 1..] {
                    let d = (a.0 - b.0) as i64 * (a.0 - b.0) as i64
                        + (a.1 - b.1) as i64 * (a.1 - b.1) as i64;
                    if d > best {
                        best = d;
                        axis_ends = (*a, *b);
                    }
                }
            }
            let axis_len = (best as f32).sqrt();
            let (ax, az) = (axis_ends.0 .0 as f32, axis_ends.0 .1 as f32);
            let (dir_x, dir_z) = if axis_len > 0.0 {
                (
                    (axis_ends.1 .0 - axis_ends.0 .0) as f32 / axis_len,
                    (axis_ends.1 .1 - axis_ends.0 .1) as f32 / axis_len,
                )
            } else {
                (0.0, 0.0)
            };
            let along = |x: i32, z: i32| (x as f32 - ax) * dir_x + (z as f32 - az) * dir_z;
            let us: Vec<Vec<f32>> = cells
                .iter()
                .map(|cs| cs.iter().map(|&(x, z)| along(x, z)).collect())
                .collect();
            // A loop or hairpin maps two places onto one axis position; keep it level.
            let follows_axis = axis_len >= 1.0
                && group_indices.iter().enumerate().all(|(m, &idx)| {
                    is_covered(idx) || is_monotonic(&us[m], PROFILE_MONOTONIC_TOLERANCE)
                });

            // Requirements the deck must meet or clear, as (axis position, deck Y).
            let mut reqs: Vec<(f32, f32)> = Vec::new();
            // Where the deck meets the terrain, as (axis position, terrain Y).
            let mut anchors: Vec<(f32, i32)> = Vec::new();
            // Free ends (joints, tagged ramps) hold the deck's working level.
            let mut free_end_us: Vec<f32> = Vec::new();
            let mut raised_top = terrain_max;
            for (m, &idx) in group_indices.iter().enumerate() {
                if is_covered(idx) {
                    continue;
                }
                let (s, e) = ends(idx);
                let last = cells[m].len() - 1;
                for (xz, i) in [(s, 0), (e, last)] {
                    if anchors_end(xz) {
                        anchors.push((us[m][i], terrain[m][i]));
                        reqs.push((us[m][i], terrain[m][i] as f32));
                    } else if let Some(&joint_y) = resolved_joints.get(&xz) {
                        // Meet the structure already built on the other side of the joint.
                        anchors.push((us[m][i], joint_y));
                        reqs.push((us[m][i], joint_y as f32));
                        raised_top = raised_top.max(joint_y);
                    } else if is_boundary(xz) && !covered_boundaries.contains(&xz) {
                        free_end_us.push(us[m][i]);
                    }
                }
            }
            let anchor_us: Vec<f32> = anchors.iter().map(|a| a.0).collect();
            // Caps a clearance so the deck can still ramp down to every anchor.
            let reachable = |u: f32| {
                anchors
                    .iter()
                    .map(|&(ua, ya)| ya as f32 + (u - ua).abs() * MAX_CLEARANCE_SLOPE)
                    .fold(f32::INFINITY, f32::min)
            };
            let mut level_deck = terrain_max;
            let mut connected: HashMap<usize, bool> = HashMap::new();
            for (m, &idx) in group_indices.iter().enumerate() {
                if is_covered(idx) {
                    continue;
                }
                let layer = effective_layer(bridge_ways[idx]);
                let path = &cells[m];
                for (i, &(x, z)) in path.iter().enumerate() {
                    let u = us[m][i];
                    let ground = terrain[m][i];
                    reqs.push((u, ground as f32));
                    let mut need = ground;
                    let (px, pz) = path[i.saturating_sub(1)];
                    let (nx, nz) = path[(i + 1).min(path.len() - 1)];
                    let heading = ((nx - px) as f32, (nz - pz) as f32);
                    for seg in obstacles.at(x, z) {
                        if dist_to_segment(x as f32, z as f32, seg) > seg.reach
                            || !crosses(heading, seg)
                        {
                            continue;
                        }
                        // Approach roads meeting the structure run at its level, not under it.
                        let touches = *connected.entry(seg.way).or_insert_with(|| {
                            obstacles.ways[seg.way]
                                .nodes
                                .iter()
                                .any(|n| group_nodes.contains(&(n.x, n.z)))
                        });
                        if !touches {
                            need = need.max(ground + seg.headroom);
                        }
                    }
                    if let Some(&(lower_y, lower_layer)) = resolved_decks.get(&(x, z)) {
                        if lower_layer < layer {
                            need = need.max(lower_y + STACKED_DECK_HEADROOM);
                        }
                    }
                    if need > ground {
                        level_deck = level_deck.max(need);
                        let capped = (need as f32).min(reachable(u)).max(ground as f32);
                        reqs.push((u, capped));
                        raised_top = raised_top.max(capped.round() as i32);
                    }
                }
            }

            // Long flat spans rise to their layer, arches to their curve.
            let has_boundary_endpoint = group_indices.iter().any(|&idx| {
                let (s, e) = ends(idx);
                is_boundary(s) || is_boundary(e)
            });
            let flat_span = has_boundary_endpoint
                && dip < FLAT_TERRAIN_DIP_THRESHOLD
                && total_length >= SHORT_BRIDGE_LENGTH_BLOCKS;
            let mut clearance = if flat_span {
                max_layer * LAYER_HEIGHT_STEP
            } else {
                0
            };
            if group_style == BridgeStyle::Arch && flat_span {
                clearance = clearance.max(ARCH_MIN_CLEARANCE);
            }
            if clearance > 0 {
                let plateau = terrain_max + clearance;
                level_deck = level_deck.max(plateau);
                let (u_min, u_max) = us
                    .iter()
                    .flatten()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), &u| (lo.min(u), hi.max(u)));
                let span = (u_max - u_min).max(0.0);
                let ramp = (span * 0.35).clamp(15.0, 50.0).min(span / 2.0);
                let mut placed = false;
                for (m, &idx) in group_indices.iter().enumerate() {
                    if is_covered(idx) {
                        continue;
                    }
                    for &u in &us[m] {
                        if anchor_us.iter().all(|&a| (u - a).abs() >= ramp) {
                            reqs.push((u, plateau as f32));
                            placed = true;
                        }
                    }
                }
                if !placed {
                    reqs.push(((u_min + u_max) / 2.0, plateau as f32));
                }
                raised_top = raised_top.max(plateau);
            }
            for &u in &free_end_us {
                reqs.push((u, raised_top as f32));
            }

            let hull = if follows_axis {
                upper_hull(reqs)
            } else {
                Vec::new()
            };
            let mut member_ys: Vec<Vec<i32>> = us
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|&u| {
                            if follows_axis {
                                hull_at(&hull, u).round() as i32
                            } else {
                                level_deck
                            }
                        })
                        .collect()
                })
                .collect();

            // Ends below the profile ramp up inside the member.
            for (m, &idx) in group_indices.iter().enumerate() {
                let ys = &mut member_ys[m];
                let total = ys.len();
                if is_covered(idx) || total < 2 {
                    continue;
                }
                let (s, e) = ends(idx);
                let ground_s = terrain[m][0];
                let ground_e = terrain[m][total - 1];
                let ramp_s = anchors_end(s) && ys[0] > ground_s + 1;
                let ramp_e = anchors_end(e) && ys[total - 1] > ground_e + 1;
                let cap = if ramp_s && ramp_e {
                    (total / 2).max(1)
                } else {
                    total - 1
                };
                let len = ((total as f32 * 0.35).clamp(15.0, 50.0) as usize).clamp(1, cap);
                if ramp_s {
                    let top = ys[len];
                    for (k, y) in ys.iter_mut().enumerate().take(len) {
                        let t = k as f32 / len as f32;
                        *y = (*y)
                            .min((ground_s as f32 + (top - ground_s) as f32 * t).round() as i32);
                    }
                }
                if ramp_e {
                    let top = ys[total - 1 - len];
                    for k in 0..len {
                        let t = k as f32 / len as f32;
                        let y = &mut ys[total - 1 - k];
                        *y = (*y)
                            .min((ground_e as f32 + (top - ground_e) as f32 * t).round() as i32);
                    }
                }
                // Never bury a ramp in a slope it climbs past.
                for (y, &g) in ys.iter_mut().zip(&terrain[m]) {
                    *y = (*y).max(g);
                }
            }

            for (candidate_id, bridge_side_at_start, ground_y, xz) in pending_ramps {
                let deck_y = group_indices
                    .iter()
                    .enumerate()
                    .find_map(|(m, &idx)| {
                        let (s, e) = ends(idx);
                        if s == xz {
                            member_ys[m].first().copied()
                        } else if e == xz {
                            member_ys[m].last().copied()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(level_deck);
                ramps.insert(
                    candidate_id,
                    BridgeRampInfo {
                        bridge_side_at_start,
                        deck_y,
                        ground_y,
                    },
                );
            }

            // One pylon set per cable structure, from its longest way.
            let mut cable_carriers: Vec<usize> = Vec::new();
            let mut pylon_points: Vec<(i32, i32)> = Vec::new();
            if matches!(
                group_style,
                BridgeStyle::Suspension | BridgeStyle::CableStayed
            ) {
                let main = group_indices
                    .iter()
                    .enumerate()
                    .filter(|&(_, &i)| !is_covered(i))
                    .max_by_key(|&(_, &i)| {
                        (
                            way_length_blocks(bridge_ways[i]),
                            std::cmp::Reverse(bridge_ways[i].id),
                        )
                    });
                if let Some((m, &main_idx)) = main {
                    let path = &cells[m];
                    pylon_points = default_pylons(group_style, path.len(), true, true)
                        .into_iter()
                        .map(|i| path[i])
                        .collect();
                    let main_len = way_length_blocks(bridge_ways[main_idx]);
                    let main_heading = heading_deg(bridge_ways[main_idx]);
                    if !pylon_points.is_empty() {
                        cable_carriers = group_indices
                            .iter()
                            .copied()
                            .filter(|&i| {
                                let way = bridge_ways[i];
                                i == main_idx
                                    || (!is_covered(i)
                                        && way_length_blocks(way) * 10 >= main_len * 6
                                        && resolve_bridge_style_with_outline(way, outlines)
                                            == group_style
                                        && matches!(
                                            (heading_deg(way), main_heading),
                                            (Some(a), Some(b)) if headings_parallel(a, b)
                                        ))
                            })
                            .collect();
                    }
                }
            }

            // Populate per-member info.
            for (m, &idx) in group_indices.iter().enumerate() {
                let way = bridge_ways[idx];
                let (start_xz, end_xz) = ends(idx);
                let covered_by_wider = is_covered(idx);
                // Footways beside a road module keep their own narrow deck.
                let module_idx = (structure_has_module
                    && !covered_by_wider
                    && !is_non_vehicular_bridge_highway(
                        way.tags.get("highway").map(String::as_str).unwrap_or(""),
                    ))
                .then(|| {
                    let bump = widened_ids.contains(&way.id) as i32;
                    bridge_modules::pick_module_index(member_range(idx) + bump, total_length)
                })
                .flatten();
                if !covered_by_wider {
                    for (xz, y) in [
                        (start_xz, member_ys[m].first()),
                        (end_xz, member_ys[m].last()),
                    ] {
                        if let Some(&y) = y {
                            let joint = resolved_joints.entry(xz).or_insert(y);
                            *joint = (*joint).max(y);
                        }
                    }
                    let layer = effective_layer(way);
                    let half = module_idx
                        .and_then(bridge_modules::module_half_width)
                        .unwrap_or_else(|| way_half_width(way, scale));
                    for (&(x, z), &y) in cells[m].iter().zip(&member_ys[m]) {
                        for dx in -half..=half {
                            for dz in -half..=half {
                                resolved_decks
                                    .entry((x + dx, z + dz))
                                    .and_modify(|e| {
                                        if y > e.0 {
                                            *e = (y, layer);
                                        }
                                    })
                                    .or_insert((y, layer));
                            }
                        }
                    }
                }
                members.insert(
                    way.id,
                    BridgeMemberInfo {
                        ys: std::mem::take(&mut member_ys[m]),
                        style: group_style,
                        start_is_group_boundary: is_boundary(start_xz),
                        end_is_group_boundary: is_boundary(end_xz),
                        module_idx,
                        covered_by_wider,
                        cable_pylons: cable_carriers.contains(&idx).then(|| pylon_points.clone()),
                    },
                );
            }
        }

        // Rail decks: one level per viaduct, clear of its layer and what it crosses.
        let mut rail_uf = UnionFind::new(rail_bridge_ways.len());
        let mut rail_ends: HashMap<((i32, i32), i32), usize> = HashMap::new();
        for (i, way) in rail_bridge_ways.iter().enumerate() {
            let layer = effective_layer(way);
            for n in [&way.nodes[0], &way.nodes[way.nodes.len() - 1]] {
                match rail_ends.entry(((n.x, n.z), layer)) {
                    std::collections::hash_map::Entry::Occupied(e) => rail_uf.union(*e.get(), i),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(i);
                    }
                }
            }
        }
        // Track along a road deck of its own layer (a tram) rides that deck.
        for way in &rail_bridge_ways {
            let path = railways::build_smoothed_centerline(way);
            let layer = effective_layer(way);
            let on_road: Vec<Option<i32>> = path
                .iter()
                .map(|&(x, z)| {
                    let mut best: Option<(i32, i32)> = None;
                    for dx in -RAIL_ON_DECK_REACH..=RAIL_ON_DECK_REACH {
                        for dz in -RAIL_ON_DECK_REACH..=RAIL_ON_DECK_REACH {
                            if let Some(&(y, l)) = resolved_decks.get(&(x + dx, z + dz)) {
                                let d = dx.abs() + dz.abs();
                                if l == layer && best.is_none_or(|(bd, _)| d < bd) {
                                    best = Some((d, y));
                                }
                            }
                        }
                    }
                    best.map(|(_, y)| y)
                })
                .collect();
            let carried = on_road.iter().filter(|y| y.is_some()).count();
            // Nearly all of it, or the rest would hang off the deck without supports.
            if path.is_empty() || carried * 10 < path.len() * 9 {
                continue;
            }
            // Fill gaps from the neighbouring cells.
            let mut ys: Vec<Option<i32>> = on_road;
            for i in 1..ys.len() {
                if ys[i].is_none() {
                    ys[i] = ys[i - 1];
                }
            }
            for i in (0..ys.len().saturating_sub(1)).rev() {
                if ys[i].is_none() {
                    ys[i] = ys[i + 1];
                }
            }
            let ys: Vec<i32> = ys
                .into_iter()
                .zip(&path)
                .map(|(y, &(x, z))| y.unwrap_or_default().max(editor.get_ground_level(x, z)))
                .collect();
            rail_decks.insert(way.id, RailDeck::Carried(ys));
        }

        let mut rail_groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, way) in rail_bridge_ways.iter().enumerate() {
            if rail_decks.contains_key(&way.id) {
                continue;
            }
            rail_groups.entry(rail_uf.find(i)).or_default().push(i);
        }
        for group in rail_groups.values() {
            let mut terrain_max = i32::MIN;
            let mut terrain_min = i32::MAX;
            let mut floor = i32::MIN;
            let mut layer = 0;
            let mut arch = false;
            for &i in group {
                let way = rail_bridge_ways[i];
                let path = railways::build_smoothed_centerline(way);
                let own_nodes: HashSet<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
                let way_layer = effective_layer(way);
                layer = layer.max(way_layer);
                arch |= resolve_bridge_style_with_outline(way, outlines) == BridgeStyle::Arch;
                for (j, &(x, z)) in path.iter().enumerate() {
                    let ground = editor.get_ground_level(x, z);
                    terrain_max = terrain_max.max(ground);
                    terrain_min = terrain_min.min(ground);
                    let (px, pz) = path[j.saturating_sub(1)];
                    let (nx, nz) = path[(j + 1).min(path.len() - 1)];
                    let heading = ((nx - px) as f32, (nz - pz) as f32);
                    for seg in obstacles.at(x, z) {
                        if dist_to_segment(x as f32, z as f32, seg) > seg.reach
                            || !crosses(heading, seg)
                            || obstacles.ways[seg.way]
                                .nodes
                                .iter()
                                .any(|n| own_nodes.contains(&(n.x, n.z)))
                        {
                            continue;
                        }
                        floor = floor.max(ground + seg.headroom);
                    }
                    if let Some(&(lower_y, lower_layer)) = resolved_decks.get(&(x, z)) {
                        if lower_layer < way_layer {
                            floor = floor.max(lower_y + STACKED_DECK_HEADROOM);
                        }
                    }
                }
            }
            if terrain_max == i32::MIN {
                continue;
            }
            // Flat spans lift clear, one level more per layer; valley spans sit on their banks.
            let deck = if terrain_max - terrain_min < railways::RAIL_BRIDGE_DIP_THRESHOLD {
                let mut clearance =
                    railways::RAIL_BRIDGE_FLAT_CLEARANCE + (layer - 1).max(0) * LAYER_HEIGHT_STEP;
                if arch {
                    clearance = clearance.max(ARCH_MIN_CLEARANCE);
                }
                terrain_max + clearance
            } else {
                terrain_max
            };
            for &i in group {
                rail_decks.insert(rail_bridge_ways[i].id, RailDeck::Level(deck.max(floor)));
            }
        }

        Self {
            members,
            ramps,
            rail_decks,
        }
    }
}

fn majority_style(
    group_indices: &[usize],
    bridge_ways: &[&ProcessedWay],
    outlines: &BridgeOutlineIndex,
) -> BridgeStyle {
    let mut counts: HashMap<BridgeStyle, usize> = HashMap::new();
    for &idx in group_indices {
        let s = resolve_bridge_style_with_outline(bridge_ways[idx], outlines);
        *counts.entry(s).or_default() += 1;
    }
    // Fixed iteration order keeps world generation deterministic on ties.
    // Non-Beam styles come first so they win ties over Beam.
    const PRIORITY: [BridgeStyle; 7] = [
        BridgeStyle::Suspension,
        BridgeStyle::CableStayed,
        BridgeStyle::Arch,
        BridgeStyle::Truss,
        BridgeStyle::Covered,
        BridgeStyle::Boardwalk,
        BridgeStyle::Beam,
    ];
    let mut best = BridgeStyle::Beam;
    let mut best_count = 0;
    for s in PRIORITY {
        let c = counts.get(&s).copied().unwrap_or(0);
        if c > best_count {
            best = s;
            best_count = c;
        }
    }
    best
}

/// Lowest and highest deck over one column; two levels differ when bridges cross.
#[derive(Clone, Copy)]
struct DeckSpan {
    low: i32,
    high: i32,
}

// (x, z) -> deck Y for every cell on a bridge surface footprint.
pub struct BridgeSurfaceMap {
    cells: HashMap<(i32, i32), DeckSpan>,
    // Columns under a deck with a road or track at grade.
    grade_crossings: HashSet<(i32, i32)>,
    // Rail viaduct deck Y per column.
    rail_decks: HashMap<(i32, i32), i32>,
}

impl BridgeSurfaceMap {
    /// Highest deck Y over (x, z).
    pub fn deck_y_at(&self, x: i32, z: i32) -> Option<i32> {
        self.cells.get(&(x, z)).map(|s| s.high)
    }

    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.cells.contains_key(&(x, z))
    }

    /// True when a deck over (x, z) lies within `tolerance` of `y`.
    pub fn deck_near(&self, x: i32, z: i32, y: i32, tolerance: i32) -> bool {
        self.cells
            .get(&(x, z))
            .is_some_and(|s| s.low <= y + tolerance && s.high >= y - tolerance)
    }

    /// True when every deck over (x, z) clears water at `water_y`, girders included.
    pub fn deck_clears(&self, x: i32, z: i32, water_y: i32) -> bool {
        self.cells
            .get(&(x, z))
            .is_some_and(|s| s.low - DECK_STRUCTURE_DEPTH > water_y)
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            cells: HashMap::new(),
            grade_crossings: HashSet::new(),
            rail_decks: HashMap::new(),
        }
    }

    /// True when a road or track runs at grade under the deck at (x, z).
    pub fn over_grade_way(&self, x: i32, z: i32) -> bool {
        self.grade_crossings.contains(&(x, z))
    }

    /// True when a support from a deck at `deck_y` would land on a road, track or lower deck.
    pub fn support_blocked(&self, x: i32, z: i32, deck_y: i32) -> bool {
        let lower = |y: i32| y <= deck_y - LOWER_DECK_GAP;
        self.grade_crossings.contains(&(x, z))
            || self.cells.get(&(x, z)).is_some_and(|s| lower(s.low))
            || self.rail_decks.get(&(x, z)).is_some_and(|&y| lower(y))
    }

    /// Top block of a deck structure in the column (x, z) itself, searched around the decks
    /// within `radius`; None when nothing in that column would hold a feature up.
    pub fn supported_top(&self, editor: &WorldEditor, x: i32, z: i32, radius: i32) -> Option<i32> {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if let Some(s) = self.cells.get(&(x + dx, z + dz)) {
                    lo = lo.min(s.low);
                    hi = hi.max(s.high);
                }
            }
        }
        if lo > hi {
            return None;
        }
        editor.highest_block_between(x, z, lo - DECK_STRUCTURE_DEPTH, hi)
    }

    // Highest deck Y within `radius`; lets side features mapped just off the deck ride the bridge.
    pub fn nearby_deck_y(&self, x: i32, z: i32, radius: i32) -> Option<i32> {
        if let Some(s) = self.cells.get(&(x, z)) {
            return Some(s.high);
        }
        let mut found: Option<i32> = None;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx == 0 && dz == 0 {
                    continue;
                }
                if let Some(s) = self.cells.get(&(x + dx, z + dz)) {
                    found = Some(found.map_or(s.high, |f| f.max(s.high)));
                }
            }
        }
        found
    }

    pub fn build(
        elements: &[ProcessedElement],
        structures: &BridgeStructureMap,
        scale: f64,
    ) -> Self {
        let mut cells: HashMap<(i32, i32), DeckSpan> = HashMap::new();
        for elem in elements {
            let ProcessedElement::Way(way) = elem else {
                continue;
            };
            if way.nodes.len() < 2 {
                continue;
            }
            let Some(highway_type) = way.tags.get("highway") else {
                continue;
            };

            let member = structures.lookup_member(way.id);
            // Covered ways render nothing; keep phantom decks out of this map.
            if member.is_some_and(|m| m.covered_by_wider) {
                continue;
            }
            let ramp = structures.lookup_ramp(way.id).copied();
            if member.is_none() && ramp.is_none() {
                continue;
            }

            // Module decks are wider than the road; register their real footprint.
            let block_range = member
                .and_then(|m| m.module_idx)
                .and_then(bridge_modules::module_half_width)
                .unwrap_or_else(|| highway_block_range(highway_type, &way.tags, scale));

            let path = way_cells(way);
            let total_bresenham = path.len();
            for (tds, &(cx, cz)) in path.iter().enumerate() {
                let cell_y = match (member, ramp) {
                    (Some(info), _) => info.y_at(tds),
                    (None, Some(info)) => info.y_at(tds, total_bresenham),
                    (None, None) => continue,
                };
                for dx in -block_range..=block_range {
                    for dz in -block_range..=block_range {
                        cells
                            .entry((cx + dx, cz + dz))
                            .and_modify(|s| {
                                s.low = s.low.min(cell_y);
                                s.high = s.high.max(cell_y);
                            })
                            .or_insert(DeckSpan {
                                low: cell_y,
                                high: cell_y,
                            });
                    }
                }
            }
        }

        // Rail viaducts are rendered on their own, but still shelter what runs beneath them.
        let mut rail_decks: HashMap<(i32, i32), i32> = HashMap::new();
        for elem in elements {
            if let ProcessedElement::Way(way) = elem {
                let Some(RailDeck::Level(y)) = structures.rail_deck(way.id) else {
                    continue;
                };
                for (x, z) in railways::build_smoothed_centerline(way) {
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            let deck = rail_decks.entry((x + dx, z + dz)).or_insert(*y);
                            *deck = (*deck).min(*y);
                        }
                    }
                }
            }
        }

        let mut grade_crossings: HashSet<(i32, i32)> = HashSet::new();
        if !cells.is_empty() || !rail_decks.is_empty() {
            // Coarse occupancy so ways nowhere near a deck skip the per-cell stamping.
            let sheltered =
                |x: i32, z: i32| cells.contains_key(&(x, z)) || rail_decks.contains_key(&(x, z));
            let mut buckets: HashSet<(i32, i32)> = HashSet::new();
            for &(x, z) in cells.keys().chain(rail_decks.keys()) {
                buckets.insert((
                    x.div_euclid(OBSTACLE_GRID_CELL),
                    z.div_euclid(OBSTACLE_GRID_CELL),
                ));
            }
            for elem in elements {
                let ProcessedElement::Way(way) = elem else {
                    continue;
                };
                // An approach ramp is raised, not a road under the deck.
                if structures.lookup_ramp(way.id).is_some() {
                    continue;
                }
                let Some((half, _)) = grade_obstacle(way, scale) else {
                    continue;
                };
                // One cell of margin keeps piers off the kerb and the track bed.
                let reach = half + 1;
                for pair in way.nodes.windows(2) {
                    let (x0, x1) = (
                        pair[0].x.min(pair[1].x) - reach,
                        pair[0].x.max(pair[1].x) + reach,
                    );
                    let (z0, z1) = (
                        pair[0].z.min(pair[1].z) - reach,
                        pair[0].z.max(pair[1].z) + reach,
                    );
                    let near_deck = (x0.div_euclid(OBSTACLE_GRID_CELL)
                        ..=x1.div_euclid(OBSTACLE_GRID_CELL))
                        .any(|bx| {
                            (z0.div_euclid(OBSTACLE_GRID_CELL)..=z1.div_euclid(OBSTACLE_GRID_CELL))
                                .any(|bz| buckets.contains(&(bx, bz)))
                        });
                    if !near_deck {
                        continue;
                    }
                    for (x, _, z) in
                        bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z)
                    {
                        for dx in -reach..=reach {
                            for dz in -reach..=reach {
                                if sheltered(x + dx, z + dz) {
                                    grade_crossings.insert((x + dx, z + dz));
                                }
                            }
                        }
                    }
                }
            }
        }

        Self {
            cells,
            grade_crossings,
            rail_decks,
        }
    }
}

pub(crate) fn is_bridge_way(way: &ProcessedWay) -> bool {
    if way.tags.get("indoor").map(|s| s.as_str()) == Some("yes") {
        return false;
    }
    // Jet bridges are stamped as a schematic, so a deck and piers under one would bury it.
    if way.tags.get("aeroway").map(|s| s.as_str()) == Some("jet_bridge") {
        return false;
    }
    way.tags
        .get("bridge")
        .map(|v| v.as_str())
        .is_some_and(|v| v != "no")
}

fn is_ramp_candidate(way: &ProcessedWay) -> bool {
    if is_bridge_way(way) {
        return false;
    }
    if way.tags.get("indoor").map(|s| s.as_str()) == Some("yes") {
        return false;
    }
    if way.tags.get("embankment").is_some_and(|v| v != "no") {
        return true;
    }
    if way.tags.get("man_made").map(|s| s.as_str()) == Some("embankment") {
        return true;
    }
    if let Some(layer) = way.tags.get("layer").and_then(|v| v.parse::<i32>().ok()) {
        if layer >= 1 {
            return true;
        }
    }
    false
}

/// Half-width and headroom of an at-grade way a bridge must clear.
pub(crate) fn grade_obstacle(way: &ProcessedWay, scale: f64) -> Option<(i32, i32)> {
    if way.nodes.len() < 2
        || way.tags.get("area").map(String::as_str) == Some("yes")
        || way.tags.get("indoor").map(String::as_str) == Some("yes")
    {
        return None;
    }
    if let Some(highway) = way.tags.get("highway").map(String::as_str) {
        let below_ground = way.tags.get("tunnel").map(String::as_str) == Some("yes")
            || way
                .tags
                .get("level")
                .and_then(|l| l.parse::<i32>().ok())
                .is_some_and(|l| l < 0);
        if is_bridge_way(way)
            || below_ground
            || effective_layer(way) > 0
            || matches!(
                highway,
                "street_lamp"
                    | "crossing"
                    | "bus_stop"
                    | "proposed"
                    | "construction"
                    | "razed"
                    | "abandoned"
                    | "elevator"
                    | "platform"
                    | "corridor"
            )
        {
            return None;
        }
        let headroom = if is_non_vehicular_bridge_highway(highway) || highway == "track" {
            PATH_HEADROOM
        } else {
            ROAD_HEADROOM
        };
        return Some((highway_block_range(highway, &way.tags, scale), headroom));
    }
    if railways::is_at_grade_track(way) {
        let headroom = match way.tags.get("railway").map(String::as_str) {
            Some("tram") => ROAD_HEADROOM,
            Some("miniature") => PATH_HEADROOM,
            _ => RAIL_HEADROOM,
        };
        return Some((1, headroom));
    }
    None
}

/// Headroom over a waterway centerline, which the DEM often doesn't show.
fn waterway_clearance(way: &ProcessedWay) -> Option<i32> {
    if way.nodes.len() < 2
        || way.tags.get("tunnel").is_some_and(|t| t != "no")
        || way.tags.get("area").map(String::as_str) == Some("yes")
    {
        return None;
    }
    match way.tags.get("waterway").map(String::as_str)? {
        "river" | "canal" => Some(RIVER_HEADROOM),
        "stream" => Some(STREAM_HEADROOM),
        _ => None,
    }
}

/// One at-grade segment a deck may have to clear.
struct GradeSegment {
    ax: f32,
    az: f32,
    bx: f32,
    bz: f32,
    // Distance from the centerline still inside the way's footprint.
    reach: f32,
    headroom: i32,
    // Index into `GradeObstacles::ways`.
    way: usize,
}

/// Bucketed at-grade ways near any bridge, for clearance lookups along deck centerlines.
struct GradeObstacles<'a> {
    ways: Vec<&'a ProcessedWay>,
    segments: Vec<GradeSegment>,
    buckets: HashMap<(i32, i32), Vec<u32>>,
}

impl<'a> GradeObstacles<'a> {
    fn build(elements: &'a [ProcessedElement], scale: f64, bounds: (i32, i32, i32, i32)) -> Self {
        let mut out = Self {
            ways: Vec::new(),
            segments: Vec::new(),
            buckets: HashMap::new(),
        };
        let (min_x, min_z, max_x, max_z) = bounds;
        for elem in elements {
            let ProcessedElement::Way(way) = elem else {
                continue;
            };
            let Some((half, headroom)) = grade_obstacle(way, scale)
                .or_else(|| waterway_clearance(way).map(|headroom| (0, headroom)))
            else {
                continue;
            };
            let reach = half as f32 + 0.5;
            let r = half + 1;
            let mut way_idx: Option<usize> = None;
            for pair in way.nodes.windows(2) {
                let (x0, x1) = (pair[0].x.min(pair[1].x) - r, pair[0].x.max(pair[1].x) + r);
                let (z0, z1) = (pair[0].z.min(pair[1].z) - r, pair[0].z.max(pair[1].z) + r);
                if x1 < min_x || x0 > max_x || z1 < min_z || z0 > max_z {
                    continue;
                }
                let w = *way_idx.get_or_insert_with(|| {
                    out.ways.push(way);
                    out.ways.len() - 1
                });
                let seg = out.segments.len() as u32;
                out.segments.push(GradeSegment {
                    ax: pair[0].x as f32,
                    az: pair[0].z as f32,
                    bx: pair[1].x as f32,
                    bz: pair[1].z as f32,
                    reach,
                    headroom,
                    way: w,
                });
                for bx in x0.div_euclid(OBSTACLE_GRID_CELL)..=x1.div_euclid(OBSTACLE_GRID_CELL) {
                    for bz in z0.div_euclid(OBSTACLE_GRID_CELL)..=z1.div_euclid(OBSTACLE_GRID_CELL)
                    {
                        out.buckets.entry((bx, bz)).or_default().push(seg);
                    }
                }
            }
        }
        out
    }

    fn at(&self, x: i32, z: i32) -> impl Iterator<Item = &GradeSegment> {
        self.buckets
            .get(&(
                x.div_euclid(OBSTACLE_GRID_CELL),
                z.div_euclid(OBSTACLE_GRID_CELL),
            ))
            .into_iter()
            .flatten()
            .map(|&i| &self.segments[i as usize])
    }
}

/// False when the deck runs along the segment rather than across it.
fn crosses(heading: (f32, f32), seg: &GradeSegment) -> bool {
    let (sx, sz) = (seg.bx - seg.ax, seg.bz - seg.az);
    let norms = (heading.0.hypot(heading.1)) * sx.hypot(sz);
    if norms <= 0.0 {
        return true;
    }
    // cos 30°
    (heading.0 * sx + heading.1 * sz).abs() / norms < 0.866
}

fn dist_to_segment(px: f32, pz: f32, seg: &GradeSegment) -> f32 {
    let (dx, dz) = (seg.bx - seg.ax, seg.bz - seg.az);
    let len_sq = dx * dx + dz * dz;
    let t = if len_sq > 0.0 {
        (((px - seg.ax) * dx + (pz - seg.az) * dz) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cz) = (seg.ax + t * dx, seg.az + t * dz);
    ((px - cx).powi(2) + (pz - cz).powi(2)).sqrt()
}

/// Centerline cells in the renderer's order, shared nodes counted once.
pub(crate) fn way_cells(way: &ProcessedWay) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();
    for (seg_idx, pair) in way.nodes.windows(2).enumerate() {
        let line = bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z);
        let skip = usize::from(seg_idx > 0);
        cells.extend(line.into_iter().skip(skip).map(|(x, _, z)| (x, z)));
    }
    cells
}

/// Least concave majorant: clears every requirement without sagging into a gap.
fn upper_hull(mut pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)));
    let mut hull: Vec<(f32, f32)> = Vec::with_capacity(pts.len());
    for p in pts {
        if hull.last().is_some_and(|l| (l.0 - p.0).abs() < 1e-4) {
            continue;
        }
        while hull.len() >= 2 {
            let (o, a) = (hull[hull.len() - 2], hull[hull.len() - 1]);
            let cross = (a.0 - o.0) * (p.1 - o.1) - (a.1 - o.1) * (p.0 - o.0);
            if cross >= 0.0 {
                hull.pop();
            } else {
                break;
            }
        }
        hull.push(p);
    }
    hull
}

fn hull_at(hull: &[(f32, f32)], u: f32) -> f32 {
    match hull {
        [] => 0.0,
        [only] => only.1,
        [first, .., last] => {
            if u <= first.0 {
                return first.1;
            }
            if u >= last.0 {
                return last.1;
            }
            let i = hull.partition_point(|p| p.0 <= u);
            let (a, b) = (hull[i - 1], hull[i]);
            a.1 + (b.1 - a.1) * (u - a.0) / (b.0 - a.0).max(1e-6)
        }
    }
}

/// True when the sequence runs one way, allowing small back-steps of `tolerance`.
fn is_monotonic(us: &[f32], tolerance: f32) -> bool {
    let (Some(&first), Some(&last)) = (us.first(), us.last()) else {
        return true;
    };
    let rising = last >= first;
    let mut extreme = first;
    for &u in us {
        if rising {
            if u < extreme - tolerance {
                return false;
            }
            extreme = extreme.max(u);
        } else {
            if u > extreme + tolerance {
                return false;
            }
            extreme = extreme.min(u);
        }
    }
    true
}

fn effective_layer(way: &ProcessedWay) -> i32 {
    way.tags
        .get("layer")
        .and_then(|v| v.parse::<i32>().ok())
        .map(|l| l.max(0))
        .unwrap_or_else(|| if is_bridge_way(way) { 1 } else { 0 })
}

fn way_half_width(way: &ProcessedWay, scale: f64) -> i32 {
    highway_block_range(
        way.tags.get("highway").map(String::as_str).unwrap_or(""),
        &way.tags,
        scale,
    )
}

fn way_bbox(way: &ProcessedWay) -> (i32, i32, i32, i32) {
    way.nodes.iter().fold(
        (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
        |(x0, z0, x1, z1), n| (x0.min(n.x), z0.min(n.z), x1.max(n.x), z1.max(n.z)),
    )
}

fn bboxes_touch(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32), margin: i32) -> bool {
    a.0 - margin <= b.2 && b.0 - margin <= a.2 && a.1 - margin <= b.3 && b.1 - margin <= a.3
}

fn way_length_blocks(way: &ProcessedWay) -> usize {
    way.nodes
        .windows(2)
        .map(|p| {
            let dx = (p[1].x - p[0].x) as f32;
            let dz = (p[1].z - p[0].z) as f32;
            (dx * dx + dz * dz).sqrt() as usize
        })
        .sum()
}

fn centroid(way: &ProcessedWay) -> (i32, i32) {
    let n = way.nodes.len() as i64;
    if n == 0 {
        return (0, 0);
    }
    let mut sx: i64 = 0;
    let mut sz: i64 = 0;
    for node in &way.nodes {
        sx += node.x as i64;
        sz += node.z as i64;
    }
    ((sx / n) as i32, (sz / n) as i32)
}

fn headings_parallel(ha: f32, hb: f32) -> bool {
    let mut diff = (ha - hb).abs() % 360.0;
    if diff > 180.0 {
        diff = 360.0 - diff;
    }
    // Parallel or antiparallel both count — carriageways may be drawn either direction.
    diff <= DUAL_CARRIAGEWAY_HEADING_TOLERANCE_DEG
        || (180.0 - diff).abs() <= DUAL_CARRIAGEWAY_HEADING_TOLERANCE_DEG
}

/// Parallel bridge ways forming one deck, across layers only when the decks touch.
fn side_by_side(a: &ProcessedWay, b: &ProcessedWay, scale: f64) -> bool {
    let (Some(ha), Some(hb)) = (heading_deg(a), heading_deg(b)) else {
        return false;
    };
    if !headings_parallel(ha, hb) {
        return false;
    }
    let same_layer = effective_layer(a) == effective_layer(b);
    let touching = (way_half_width(a, scale) + way_half_width(b, scale) + 1) as f32
        + SIDE_DECK_EDGE_GAP_BLOCKS;
    let max_dist = if same_layer {
        touching.max(DUAL_CARRIAGEWAY_MAX_DISTANCE_BLOCKS)
    } else {
        touching
    };
    let (short, long) = if way_length_blocks(a) <= way_length_blocks(b) {
        (a, b)
    } else {
        (b, a)
    };
    let offsets: Vec<f32> = coverage_samples(short)
        .iter()
        .map(|&(px, pz)| lateral_offset_to_way(px, pz, long))
        .collect();
    let alongside = offsets.iter().filter(|&&d| d <= max_dist).count();
    if same_layer {
        // Carriageways are often split at different nodes.
        alongside * 2 > offsets.len()
    } else {
        // Fully alongside, never stacked.
        let mean = offsets.iter().sum::<f32>() / offsets.len().max(1) as f32;
        alongside == offsets.len() && mean >= 1.5
    }
}

/// Points at even arc-length fractions along the way, endpoints included.
fn coverage_samples(way: &ProcessedWay) -> Vec<(f32, f32)> {
    // Degenerate ways: test the lone point, never an empty (vacuously covered) set.
    if way.nodes.len() < 2 {
        return way.nodes.iter().map(|n| (n.x as f32, n.z as f32)).collect();
    }
    let mut cum: Vec<f32> = Vec::with_capacity(way.nodes.len());
    cum.push(0.0);
    let mut total = 0.0f32;
    for pair in way.nodes.windows(2) {
        let dx = (pair[1].x - pair[0].x) as f32;
        let dz = (pair[1].z - pair[0].z) as f32;
        total += (dx * dx + dz * dz).sqrt();
        cum.push(total);
    }
    [0.0f32, 0.25, 0.5, 0.75, 1.0]
        .iter()
        .map(|f| {
            let target = total * f;
            let mut seg = 0;
            while seg + 1 < cum.len() - 1 && cum[seg + 1] < target {
                seg += 1;
            }
            let seg_len = (cum[seg + 1] - cum[seg]).max(1e-6);
            let t = ((target - cum[seg]) / seg_len).clamp(0.0, 1.0);
            let (a, b) = (&way.nodes[seg], &way.nodes[seg + 1]);
            (
                a.x as f32 + t * (b.x - a.x) as f32,
                a.z as f32 + t * (b.z - a.z) as f32,
            )
        })
        .collect()
}

/// Minimum distance from a point to any centerline segment of the way.
fn lateral_offset_to_way(px: f32, pz: f32, way: &ProcessedWay) -> f32 {
    let mut best = f32::MAX;
    for pair in way.nodes.windows(2) {
        let (ax, az) = (pair[0].x as f32, pair[0].z as f32);
        let (bx, bz) = (pair[1].x as f32, pair[1].z as f32);
        let (dx, dz) = (bx - ax, bz - az);
        let len_sq = dx * dx + dz * dz;
        let t = if len_sq > 0.0 {
            (((px - ax) * dx + (pz - az) * dz) / len_sq).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (cx, cz) = (ax + t * dx, az + t * dz);
        best = best.min(((px - cx).powi(2) + (pz - cz).powi(2)).sqrt());
    }
    best
}

fn heading_deg(way: &ProcessedWay) -> Option<f32> {
    if way.nodes.len() < 2 {
        return None;
    }
    let s = &way.nodes[0];
    let e = &way.nodes[way.nodes.len() - 1];
    let dx = (e.x - s.x) as f32;
    let dz = (e.z - s.z) as f32;
    if dx == 0.0 && dz == 0.0 {
        return None;
    }
    Some(dz.atan2(dx).to_degrees())
}

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb;
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra;
        } else {
            self.parent[rb] = ra;
            self.rank[ra] += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::ProcessedNode;
    use crate::world_editor::WorldEditor;
    use std::collections::HashMap as StdMap;
    use std::path::PathBuf;

    fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    fn straight_bridge_way(
        id: u64,
        highway: &str,
        x1: i32,
        z1: i32,
        x2: i32,
        z2: i32,
    ) -> ProcessedWay {
        let mut tags = StdMap::new();
        tags.insert("highway".to_string(), highway.to_string());
        tags.insert("bridge".to_string(), "yes".to_string());
        ProcessedWay {
            id,
            nodes: vec![
                ProcessedNode {
                    id: id * 10 + 1,
                    tags: StdMap::new(),
                    x: x1,
                    z: z1,
                },
                ProcessedNode {
                    id: id * 10 + 2,
                    tags: StdMap::new(),
                    x: x2,
                    z: z2,
                },
            ],
            tags,
        }
    }

    #[test]
    fn pedestrian_only_bridge_structure_does_not_use_module_deck() {
        let xzbbox = XZBBox::rect_from_xz_lengths(80.0, 80.0).unwrap();
        let editor = test_editor(&xzbbox);
        let ways = vec![ProcessedElement::Way(straight_bridge_way(
            1, "footway", 10, 40, 60, 40,
        ))];
        let outlines = BridgeOutlineIndex::build(&ways);

        let structures = BridgeStructureMap::build(&ways, &editor, &outlines, 1.0);
        let member = structures.lookup_member(1).expect("member exists");

        assert!(
            member.module_idx.is_none(),
            "pedestrian-only bridges should not receive a road-style bridge module"
        );
    }

    #[test]
    fn footway_beside_a_road_module_keeps_its_own_deck() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let editor = test_editor(&xzbbox);
        let ways = vec![
            ProcessedElement::Way(straight_bridge_way(1, "primary", 10, 40, 60, 40)),
            // Longer than the road, so its module deck can't cover it.
            ProcessedElement::Way(straight_bridge_way(2, "footway", 0, 52, 95, 52)),
        ];
        let outlines = BridgeOutlineIndex::build(&ways);
        let structures = BridgeStructureMap::build(&ways, &editor, &outlines, 1.0);
        assert!(structures.lookup_member(1).unwrap().module_idx.is_some());
        let footway = structures.lookup_member(2).unwrap();
        assert!(!footway.covered_by_wider);
        assert!(footway.module_idx.is_none());
    }

    #[test]
    fn vehicular_bridge_structure_still_uses_module_deck() {
        let xzbbox = XZBBox::rect_from_xz_lengths(80.0, 80.0).unwrap();
        let editor = test_editor(&xzbbox);
        let ways = vec![ProcessedElement::Way(straight_bridge_way(
            2, "primary", 10, 40, 60, 40,
        ))];
        let outlines = BridgeOutlineIndex::build(&ways);

        let structures = BridgeStructureMap::build(&ways, &editor, &outlines, 1.0);
        let member = structures.lookup_member(2).expect("member exists");

        assert!(member.module_idx.is_some());
    }

    fn way_with(id: u64, tags: &[(&str, &str)], points: &[(i32, i32)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: points
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 100 + i as u64,
                    tags: StdMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    const ROAD_BRIDGE: &[(&str, &str)] = &[("highway", "residential"), ("bridge", "yes")];

    /// Structures over a 100x100 world whose terrain Y at column x is `height(x)`.
    fn structures_over(
        ways: &[ProcessedWay],
        height: impl Fn(usize) -> f32,
    ) -> (BridgeStructureMap, BridgeSurfaceMap) {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let row: Vec<f32> = (0..100).map(height).collect();
        let ground = crate::ground::Ground::new_elevation_test(vec![row; 100], 100, 100);
        editor.set_ground(std::sync::Arc::new(ground));
        let elements: Vec<ProcessedElement> =
            ways.iter().cloned().map(ProcessedElement::Way).collect();
        let outlines = BridgeOutlineIndex::build(&elements);
        let structures = BridgeStructureMap::build(&elements, &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&elements, &structures, 1.0);
        (structures, surface)
    }

    fn ys(structures: &BridgeStructureMap, id: u64) -> Vec<i32> {
        let member = structures.lookup_member(id).expect("member exists");
        (0..member.ys.len()).map(|i| member.y_at(i)).collect()
    }

    #[test]
    fn two_node_carriageways_side_by_side_are_one_structure() {
        // Single-segment ways: the middle node is the end node.
        let tags = &[
            ("highway", "secondary"),
            ("bridge", "yes"),
            ("oneway", "yes"),
        ];
        let a = way_with(1, tags, &[(10, 40), (70, 40)]);
        let b = way_with(2, tags, &[(70, 48), (10, 48)]);
        assert!(side_by_side(&a, &b, 1.0));
        let far = way_with(3, tags, &[(70, 90), (10, 90)]);
        assert!(!side_by_side(&a, &far, 1.0));
    }

    #[test]
    fn sidewalk_on_another_layer_shares_the_road_deck() {
        let road = way_with(
            1,
            &[("highway", "primary"), ("bridge", "yes"), ("layer", "2")],
            &[(10, 40), (80, 40)],
        );
        let walk = way_with(
            2,
            &[("highway", "footway"), ("bridge", "yes"), ("layer", "1")],
            &[(10, 48), (80, 48)],
        );
        let (structures, _) = structures_over(&[road, walk], |_| 0.0);
        assert_eq!(ys(&structures, 1)[35], 12);
        assert_eq!(ys(&structures, 2)[35], 12, "sidewalk rides the road deck");
    }

    #[test]
    fn deck_clears_a_road_passing_underneath() {
        let bridge = way_with(1, ROAD_BRIDGE, &[(10, 40), (34, 40)]);
        let road = way_with(2, &[("highway", "residential")], &[(22, 10), (22, 70)]);
        let (structures, surface) = structures_over(&[bridge, road], |_| 0.0);
        let profile = ys(&structures, 1);
        assert_eq!(profile[0], 0, "meets the ground at its ends");
        assert_eq!(*profile.last().unwrap(), 0);
        assert!(profile[12] >= ROAD_HEADROOM, "clears the road: {profile:?}");
        assert!(surface.support_blocked(22, 40, profile[12]));
        assert!(!surface.support_blocked(15, 40, profile[5]));
    }

    #[test]
    fn short_bridge_rises_no_steeper_than_a_block_per_cell() {
        let bridge = way_with(1, ROAD_BRIDGE, &[(20, 40), (26, 40)]);
        let road = way_with(2, &[("highway", "footway")], &[(23, 10), (23, 70)]);
        let (structures, _) = structures_over(&[bridge, road], |_| 0.0);
        let profile = ys(&structures, 1);
        assert!(*profile.iter().max().unwrap() <= 3, "{profile:?}");
        for pair in profile.windows(2) {
            assert!((pair[0] - pair[1]).abs() <= 1, "{profile:?}");
        }
    }

    #[test]
    fn approach_road_meeting_the_deck_does_not_lift_it() {
        let bridge = way_with(1, ROAD_BRIDGE, &[(10, 40), (34, 40)]);
        let road = way_with(
            2,
            &[("highway", "residential")],
            &[(34, 10), (34, 40), (34, 70)],
        );
        let (structures, _) = structures_over(&[bridge, road], |_| 0.0);
        assert!(ys(&structures, 1).iter().all(|&y| y == 0));
    }

    #[test]
    fn deck_spans_a_valley_instead_of_following_it() {
        let bridge = way_with(1, ROAD_BRIDGE, &[(10, 40), (70, 40)]);
        let (structures, _) =
            structures_over(
                &[bridge],
                |x| {
                    if (25..=55).contains(&x) {
                        0.0
                    } else {
                        20.0
                    }
                },
            );
        assert!(ys(&structures, 1).iter().all(|&y| y == 20));
    }

    #[test]
    fn deck_runs_straight_between_banks_of_different_height() {
        let bridge = way_with(1, ROAD_BRIDGE, &[(10, 40), (70, 40)]);
        let (structures, _) = structures_over(&[bridge], |x| match x {
            ..=15 => 10.0,
            65.. => 30.0,
            _ => 0.0,
        });
        let profile = ys(&structures, 1);
        assert_eq!(profile[0], 10);
        assert_eq!(*profile.last().unwrap(), 30);
        assert!((18..=22).contains(&profile[30]), "{profile:?}");
    }

    #[test]
    fn upper_deck_clears_the_deck_it_crosses() {
        let lower = way_with(
            1,
            &[
                ("highway", "residential"),
                ("bridge", "yes"),
                ("layer", "1"),
            ],
            &[(20, 40), (44, 40)],
        );
        let upper = way_with(
            2,
            &[
                ("highway", "residential"),
                ("bridge", "yes"),
                ("layer", "2"),
            ],
            &[(32, 20), (32, 60)],
        );
        let (structures, surface) = structures_over(&[lower, upper], |_| 0.0);
        let lower_y = ys(&structures, 1)[12];
        let upper_y = ys(&structures, 2)[20];
        assert!(
            upper_y >= lower_y + STACKED_DECK_HEADROOM,
            "{lower_y} vs {upper_y}"
        );
        assert!(surface.support_blocked(32, 40, upper_y));
    }

    #[test]
    fn joint_between_layers_stays_in_the_air() {
        let approach = way_with(
            1,
            &[("highway", "motorway"), ("bridge", "yes"), ("layer", "1")],
            &[(10, 40), (40, 40)],
        );
        let span = way_with(
            2,
            &[("highway", "motorway"), ("bridge", "yes"), ("layer", "2")],
            &[(40, 40), (70, 40)],
        );
        let road = way_with(3, &[("highway", "residential")], &[(36, 10), (36, 70)]);
        let (structures, _) = structures_over(&[approach, span, road], |_| 0.0);
        let approach_end = *ys(&structures, 1).last().unwrap();
        assert!(approach_end > 0, "the joint is not a place to touch down");
        assert_eq!(
            ys(&structures, 2)[0],
            approach_end,
            "both sides meet at the joint"
        );
    }

    #[test]
    fn approach_ramp_keeps_its_own_pillars() {
        let bridge = way_with(
            1,
            &[
                ("highway", "residential"),
                ("bridge", "yes"),
                ("layer", "1"),
            ],
            &[(40, 40), (80, 40)],
        );
        let ramp = way_with(
            2,
            &[("highway", "residential"), ("embankment", "yes")],
            &[(10, 40), (40, 40)],
        );
        let (structures, surface) = structures_over(&[bridge, ramp], |_| 0.0);
        let ramp_y = structures.lookup_ramp(2).expect("tagged ramp").y_at(20, 31);
        assert!(ramp_y > 2, "the ramp is raised there");
        assert!(!surface.support_blocked(30, 40, ramp_y));
    }

    #[test]
    fn ring_of_bridge_ways_stays_grounded() {
        let tags = &[("highway", "footway"), ("bridge", "yes"), ("layer", "1")];
        let a = way_with(1, tags, &[(20, 20), (60, 20), (60, 60)]);
        let b = way_with(2, tags, &[(60, 60), (20, 60), (20, 20)]);
        let (structures, _) = structures_over(&[a, b], |_| 0.0);
        assert!(ys(&structures, 1).iter().all(|&y| y == 0));
    }

    #[test]
    fn rail_viaduct_blocks_road_piers_above_it() {
        let rail = way_with(
            1,
            &[("railway", "rail"), ("bridge", "yes"), ("layer", "1")],
            &[(10, 40), (70, 40)],
        );
        let road = way_with(
            2,
            &[
                ("highway", "residential"),
                ("bridge", "yes"),
                ("layer", "2"),
            ],
            &[(40, 10), (40, 70)],
        );
        let (structures, surface) = structures_over(&[rail, road], |_| 0.0);
        let Some(RailDeck::Level(rail_y)) = structures.rail_deck(1) else {
            panic!("rail viaduct has its own deck");
        };
        assert!(surface.support_blocked(40, 40, rail_y + STACKED_DECK_HEADROOM));
        assert!(!surface.support_blocked(40, 40, *rail_y));
    }

    #[test]
    fn tram_on_a_road_bridge_rides_its_deck() {
        let road = way_with(
            1,
            &[("highway", "primary"), ("bridge", "yes"), ("layer", "1")],
            &[(10, 40), (70, 40)],
        );
        let tram = way_with(
            2,
            &[("railway", "tram"), ("bridge", "yes"), ("layer", "1")],
            &[(10, 41), (70, 41)],
        );
        let (structures, _) = structures_over(&[road, tram], |_| 0.0);
        let road_mid = ys(&structures, 1)[30];
        match structures.rail_deck(2) {
            Some(RailDeck::Carried(rail)) => assert_eq!(rail[30], road_mid),
            _ => panic!("tram should ride the road deck"),
        }
    }

    #[test]
    fn rail_viaduct_split_into_ways_keeps_one_level() {
        let tags = &[("railway", "rail"), ("bridge", "viaduct")];
        let a = way_with(1, tags, &[(10, 40), (40, 40)]);
        let b = way_with(2, tags, &[(40, 40), (70, 40)]);
        let (structures, _) = structures_over(&[a, b], |x| if x > 50 { 12.0 } else { 0.0 });
        let level = |id| match structures.rail_deck(id) {
            Some(RailDeck::Level(y)) => *y,
            _ => panic!("rail bridge {id} has its own deck"),
        };
        assert_eq!(level(1), level(2));
        assert!(level(1) >= 12);
    }
}
