use crate::bresenham::bresenham_line;
use crate::element_processing::highways::highway_block_range;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};

const LAYER_HEIGHT_STEP: i32 = 6;
const FLAT_TERRAIN_DIP_THRESHOLD: i32 = 4;
const SHORT_BRIDGE_LENGTH_BLOCKS: usize = 30;
const BRIDGE_NAME_FUSE_DISTANCE_BLOCKS: i32 = 200;
const DUAL_CARRIAGEWAY_MAX_DISTANCE_BLOCKS: f32 = 12.0;
const DUAL_CARRIAGEWAY_HEADING_TOLERANCE_DEG: f32 = 20.0;
const CENTERLINE_SAMPLE_LIMIT: usize = 5;

#[derive(Clone, Copy)]
pub struct BridgeMemberInfo {
    pub deck_y: i32,
    // Some(terrain_y) = ramp from that terrain up to deck_y at this endpoint.
    pub start_internal_ramp: Option<i32>,
    pub end_internal_ramp: Option<i32>,
}

impl BridgeMemberInfo {
    pub fn y_at(&self, tds: usize, total_bresenham: usize, ramp_length: usize) -> i32 {
        if total_bresenham == 0 {
            return self.deck_y;
        }
        let last_idx = total_bresenham - 1;
        let denom = ramp_length.saturating_sub(1).max(1) as f32;

        if let Some(start_ground_y) = self.start_internal_ramp {
            if tds < ramp_length {
                let t = (tds as f32 / denom).min(1.0);
                let span = (self.deck_y - start_ground_y) as f32;
                return (start_ground_y as f32 + span * t).round() as i32;
            }
        }
        let dist_from_end = last_idx.saturating_sub(tds);
        if let Some(end_ground_y) = self.end_internal_ramp {
            if dist_from_end < ramp_length {
                let t = (dist_from_end as f32 / denom).min(1.0);
                let span = (self.deck_y - end_ground_y) as f32;
                return (end_ground_y as f32 + span * t).round() as i32;
            }
        }
        self.deck_y
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

pub struct BridgeStructureMap {
    members: HashMap<u64, BridgeMemberInfo>,
    ramps: HashMap<u64, BridgeRampInfo>,
}

impl BridgeStructureMap {
    pub fn lookup_member(&self, way_id: u64) -> Option<&BridgeMemberInfo> {
        self.members.get(&way_id)
    }

    pub fn lookup_ramp(&self, way_id: u64) -> Option<&BridgeRampInfo> {
        self.ramps.get(&way_id)
    }

    pub fn build(elements: &[ProcessedElement], editor: &WorldEditor) -> Self {
        let mut bridge_ways: Vec<&ProcessedWay> = Vec::new();
        let mut other_highway_ways: Vec<&ProcessedWay> = Vec::new();
        for elem in elements {
            if let ProcessedElement::Way(w) = elem {
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

        if bridge_ways.is_empty() {
            return Self {
                members: HashMap::new(),
                ramps: HashMap::new(),
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

        // Step 3: union dual carriageways (parallel one-way bridge ways with overlapping spans).
        let mut oneway_indices: Vec<usize> = Vec::new();
        for (i, way) in bridge_ways.iter().enumerate() {
            if is_oneway(way) {
                oneway_indices.push(i);
            }
        }
        for ai in 0..oneway_indices.len() {
            for bi in (ai + 1)..oneway_indices.len() {
                let a = oneway_indices[ai];
                let b = oneway_indices[bi];
                if uf.find(a) == uf.find(b) {
                    continue;
                }
                if effective_layer(bridge_ways[a]) != effective_layer(bridge_ways[b]) {
                    continue;
                }
                if are_dual_carriageway_pair(bridge_ways[a], bridge_ways[b]) {
                    uf.union(a, b);
                }
            }
        }

        // Group bridge ways by root.
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..bridge_ways.len() {
            groups.entry(uf.find(i)).or_default().push(i);
        }

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

        let mut members: HashMap<u64, BridgeMemberInfo> = HashMap::new();
        let mut ramps: HashMap<u64, BridgeRampInfo> = HashMap::new();
        // Track ramp ways to make sure each ramp attaches to only one structure.
        let mut claimed_ramp_ways: HashSet<u64> = HashSet::new();

        for group_indices in groups.values() {
            // Count endpoint occurrences across the group to identify boundary vs internal.
            let mut endpoint_counts: HashMap<(i32, i32), usize> = HashMap::new();
            for &idx in group_indices {
                let way = bridge_ways[idx];
                let s = &way.nodes[0];
                let e = &way.nodes[way.nodes.len() - 1];
                *endpoint_counts.entry((s.x, s.z)).or_default() += 1;
                let end_xz = (e.x, e.z);
                if end_xz != (s.x, s.z) {
                    *endpoint_counts.entry(end_xz).or_default() += 1;
                }
            }

            // Effective layer for this structure: max of members; default 1 if any member has bridge tag and no layer.
            let mut max_layer = 0;
            let mut had_unlabelled = false;
            for &idx in group_indices {
                let way = bridge_ways[idx];
                let raw = way.tags.get("layer").and_then(|v| v.parse::<i32>().ok());
                match raw {
                    Some(l) => max_layer = max_layer.max(l.max(0)),
                    None => had_unlabelled = true,
                }
            }
            if max_layer == 0 && had_unlabelled {
                max_layer = 1;
            }

            // Sample terrain Ys: every endpoint plus a small set of midpoints across the group.
            let mut terrain_samples: Vec<i32> = Vec::new();
            for &idx in group_indices {
                let way = bridge_ways[idx];
                for sample in centerline_samples(way) {
                    terrain_samples.push(editor.get_ground_level(sample.0, sample.1));
                }
            }
            if terrain_samples.is_empty() {
                continue;
            }
            let terrain_max = *terrain_samples.iter().max().unwrap();
            let terrain_min = *terrain_samples.iter().min().unwrap();
            let dip = terrain_max - terrain_min;
            let total_length: usize = group_indices
                .iter()
                .map(|&i| way_length_blocks(bridge_ways[i]))
                .sum();
            let clearance =
                if dip < FLAT_TERRAIN_DIP_THRESHOLD && total_length >= SHORT_BRIDGE_LENGTH_BLOCKS {
                    max_layer * LAYER_HEIGHT_STEP
                } else {
                    0
                };
            let deck_y = terrain_max + clearance;

            let mut boundary_with_external_ramp: HashMap<(i32, i32), bool> = HashMap::new();
            for (&xz, &count) in &endpoint_counts {
                if count > 1 {
                    continue;
                }
                let other_indices = match node_to_other_highways.get(&xz) {
                    Some(v) => v,
                    None => {
                        boundary_with_external_ramp.insert(xz, false);
                        continue;
                    }
                };
                let mut found_ramp = false;
                for &oi in other_indices {
                    let candidate = other_highway_ways[oi];
                    if !is_ramp_candidate(candidate) {
                        continue;
                    }
                    // Each ramp can only attach once; if already claimed, skip.
                    if claimed_ramp_ways.contains(&candidate.id) {
                        continue;
                    }
                    let bridge_side_at_start = (candidate.nodes[0].x, candidate.nodes[0].z) == xz;
                    let far_node = if bridge_side_at_start {
                        &candidate.nodes[candidate.nodes.len() - 1]
                    } else {
                        &candidate.nodes[0]
                    };
                    // Reject if the far end is also a bridge-structure boundary (connector between two bridges).
                    if endpoint_counts.contains_key(&(far_node.x, far_node.z)) {
                        continue;
                    }
                    let ground_y = editor.get_ground_level(far_node.x, far_node.z);
                    let info = BridgeRampInfo {
                        bridge_side_at_start,
                        deck_y,
                        ground_y,
                    };
                    ramps.insert(candidate.id, info);
                    claimed_ramp_ways.insert(candidate.id);
                    found_ramp = true;
                }
                boundary_with_external_ramp.insert(xz, found_ramp);
            }

            // Populate per-member info.
            for &idx in group_indices {
                let way = bridge_ways[idx];
                let s = &way.nodes[0];
                let e = &way.nodes[way.nodes.len() - 1];
                let start_xz = (s.x, s.z);
                let end_xz = (e.x, e.z);

                let start_internal_ramp = decide_internal_ramp(
                    start_xz,
                    deck_y,
                    &endpoint_counts,
                    &boundary_with_external_ramp,
                    editor,
                );
                let end_internal_ramp = decide_internal_ramp(
                    end_xz,
                    deck_y,
                    &endpoint_counts,
                    &boundary_with_external_ramp,
                    editor,
                );
                members.insert(
                    way.id,
                    BridgeMemberInfo {
                        deck_y,
                        start_internal_ramp,
                        end_internal_ramp,
                    },
                );
            }
        }

        Self { members, ramps }
    }
}

fn decide_internal_ramp(
    xz: (i32, i32),
    deck_y: i32,
    endpoint_counts: &HashMap<(i32, i32), usize>,
    boundary_with_external_ramp: &HashMap<(i32, i32), bool>,
    editor: &WorldEditor,
) -> Option<i32> {
    // Internal node (shared with another member): no ramp.
    if endpoint_counts.get(&xz).copied().unwrap_or(0) > 1 {
        return None;
    }
    // External ramp claimed: no internal ramp.
    if boundary_with_external_ramp
        .get(&xz)
        .copied()
        .unwrap_or(false)
    {
        return None;
    }
    let ground_y = editor.get_ground_level(xz.0, xz.1);
    if deck_y > ground_y {
        Some(ground_y)
    } else {
        None
    }
}

// (x, z) -> deck Y for every cell on a bridge surface footprint.
pub struct BridgeSurfaceMap {
    cells: HashMap<(i32, i32), i32>,
}

impl BridgeSurfaceMap {
    pub fn deck_y_at(&self, x: i32, z: i32) -> Option<i32> {
        self.cells.get(&(x, z)).copied()
    }

    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.cells.contains_key(&(x, z))
    }

    // Highest deck Y within `radius`; lets side features mapped just off the deck ride the bridge.
    pub fn nearby_deck_y(&self, x: i32, z: i32, radius: i32) -> Option<i32> {
        if let Some(&y) = self.cells.get(&(x, z)) {
            return Some(y);
        }
        let mut found: Option<i32> = None;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx == 0 && dz == 0 {
                    continue;
                }
                if let Some(&y) = self.cells.get(&(x + dx, z + dz)) {
                    found = Some(found.map_or(y, |f| f.max(y)));
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
        let mut cells: HashMap<(i32, i32), i32> = HashMap::new();
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

            let member = structures.lookup_member(way.id).copied();
            let ramp = structures.lookup_ramp(way.id).copied();
            if member.is_none() && ramp.is_none() {
                continue;
            }

            let block_range = highway_block_range(highway_type, &way.tags, scale);

            let total_bresenham: usize = way
                .nodes
                .windows(2)
                .map(|p| {
                    let dx = (p[1].x - p[0].x).unsigned_abs() as usize;
                    let dz = (p[1].z - p[0].z).unsigned_abs() as usize;
                    dx.max(dz)
                })
                .sum::<usize>()
                + 1;
            let internal_ramp_length: usize = {
                let raw = (total_bresenham as f32 * 0.35).clamp(15.0, 50.0) as usize;
                let cap = (total_bresenham / 2).max(1);
                raw.clamp(1, cap)
            };

            let mut tds: usize = 0;
            for (seg_idx, window) in way.nodes.windows(2).enumerate() {
                let bp = bresenham_line(window[0].x, 0, window[0].z, window[1].x, 0, window[1].z);
                let skip_first = if seg_idx == 0 { 0 } else { 1 };
                for (cx, _, cz) in bp.iter().skip(skip_first) {
                    let cell_y = if let Some(info) = member {
                        info.y_at(tds, total_bresenham, internal_ramp_length)
                    } else if let Some(info) = ramp {
                        info.y_at(tds, total_bresenham)
                    } else {
                        tds += 1;
                        continue;
                    };
                    for dx in -block_range..=block_range {
                        for dz in -block_range..=block_range {
                            let key = (*cx + dx, *cz + dz);
                            // Keep the higher deck on overlap so node features ride the upper level.
                            cells
                                .entry(key)
                                .and_modify(|existing| {
                                    if cell_y > *existing {
                                        *existing = cell_y;
                                    }
                                })
                                .or_insert(cell_y);
                        }
                    }
                    tds += 1;
                }
            }
        }
        Self { cells }
    }
}

fn is_bridge_way(way: &ProcessedWay) -> bool {
    if way.tags.get("indoor").map(|s| s.as_str()) == Some("yes") {
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

fn effective_layer(way: &ProcessedWay) -> i32 {
    way.tags
        .get("layer")
        .and_then(|v| v.parse::<i32>().ok())
        .map(|l| l.max(0))
        .unwrap_or_else(|| if is_bridge_way(way) { 1 } else { 0 })
}

fn is_oneway(way: &ProcessedWay) -> bool {
    matches!(
        way.tags.get("oneway").map(|s| s.as_str()),
        Some("yes") | Some("-1") | Some("true")
    )
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

// Sample endpoints + a few interior nodes at evenly spaced indices.
fn centerline_samples(way: &ProcessedWay) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    if way.nodes.is_empty() {
        return out;
    }
    let last = way.nodes.len() - 1;
    out.push((way.nodes[0].x, way.nodes[0].z));
    if last > 0 {
        out.push((way.nodes[last].x, way.nodes[last].z));
    }
    if last >= 2 {
        let interior = last - 1;
        let take = interior.min(CENTERLINE_SAMPLE_LIMIT);
        if take > 0 {
            let step = (interior.max(1)) / take.max(1);
            let step = step.max(1);
            let mut idx = 1;
            while idx < last && out.len() < CENTERLINE_SAMPLE_LIMIT + 2 {
                out.push((way.nodes[idx].x, way.nodes[idx].z));
                idx += step;
            }
        }
    }
    out
}

fn are_dual_carriageway_pair(a: &ProcessedWay, b: &ProcessedWay) -> bool {
    let ha = heading_deg(a);
    let hb = heading_deg(b);
    let (Some(ha), Some(hb)) = (ha, hb) else {
        return false;
    };
    let mut diff = (ha - hb).abs() % 360.0;
    if diff > 180.0 {
        diff = 360.0 - diff;
    }
    // Parallel or antiparallel both count — carriageways may be drawn either direction.
    let parallel = diff <= DUAL_CARRIAGEWAY_HEADING_TOLERANCE_DEG
        || (180.0 - diff).abs() <= DUAL_CARRIAGEWAY_HEADING_TOLERANCE_DEG;
    if !parallel {
        return false;
    }
    let mid_a = midpoint(a);
    let mid_b = midpoint(b);
    let dx = (mid_a.0 - mid_b.0) as f32;
    let dz = (mid_a.1 - mid_b.1) as f32;
    let dist = (dx * dx + dz * dz).sqrt();
    dist <= DUAL_CARRIAGEWAY_MAX_DISTANCE_BLOCKS
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

fn midpoint(way: &ProcessedWay) -> (i32, i32) {
    let mid = way.nodes.len() / 2;
    let n = &way.nodes[mid];
    (n.x, n.z)
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
