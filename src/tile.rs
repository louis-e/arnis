//! Tile subdivision and element assignment for parallel world generation.
//!
//! Divides the world bounding box into fixed-size tiles (default 512×512 blocks,
//! aligned with Minecraft region boundaries). Each tile can be processed independently
//! on a separate CPU core.

use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::{ProcessedElement, ProcessedRelation, ProcessedWay};
use std::collections::HashMap;

/// Bounds of a single tile within the world.
#[derive(Clone, Debug)]
pub struct TileBounds {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32, // exclusive
    pub max_z: i32, // exclusive
}

impl TileBounds {
    /// Check if a point is within the strict tile bounds.
    #[inline]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x < self.max_x && z >= self.min_z && z < self.max_z
    }

    /// Return expanded bounds with a halo zone.
    pub fn expanded(&self, halo: i32) -> TileBounds {
        TileBounds {
            min_x: self.min_x - halo,
            min_z: self.min_z - halo,
            max_x: self.max_x + halo,
            max_z: self.max_z + halo,
        }
    }
}

/// Default tile size (512x512 = 1 Minecraft region = 32x32 chunks of 16 blocks each)
pub const DEFAULT_TILE_SIZE: i32 = 512;

/// Halo on each side of a tile editor's xzbbox during parallel processing.
///
/// Must be >= the maximum half-width of any element rendered into a tile so
/// that elements assigned by centroid (buildings, areas) can extend across
/// the strict tile boundary into the halo without being clipped by the
/// editor's silently-drop-out-of-bbox check. 64 covers all realistic
/// buildings, runways and similar; if you raise it, peak per-tile memory
/// scales linearly.
pub const TILE_EDITOR_HALO: i32 = 64;

/// Widest rendered half-width of any linear element (aeroway runways), in metres.
const MAX_LINEAR_HALF_WIDTH_M: f64 = 40.0;

/// Subdivide the world bounding box into tiles of the given size.
/// Tiles at the edge may be smaller than the full tile size.
pub fn create_tiles(xzbbox: &XZBBox, tile_size: i32) -> Vec<TileBounds> {
    let mut tiles = Vec::new();

    // Align tile grid to region boundaries (multiples of 512 from world origin)
    // This ensures each tile maps cleanly to Minecraft regions
    let aligned_min_x = (xzbbox.min_x() >> 9) << 9; // floor to nearest 512
    let aligned_min_z = (xzbbox.min_z() >> 9) << 9;
    let aligned_max_x = ((xzbbox.max_x() + 512) >> 9) << 9; // ceil to nearest 512 region
    let aligned_max_z = ((xzbbox.max_z() + 512) >> 9) << 9;

    let mut z = aligned_min_z;
    while z < aligned_max_z {
        let mut x = aligned_min_x;
        while x < aligned_max_x {
            let tile_max_x = (x + tile_size).min(aligned_max_x);
            let tile_max_z = (z + tile_size).min(aligned_max_z);

            // Only create a tile if it overlaps with the actual world bbox
            if tile_max_x > xzbbox.min_x()
                && x <= xzbbox.max_x()
                && tile_max_z > xzbbox.min_z()
                && z <= xzbbox.max_z()
            {
                tiles.push(TileBounds {
                    min_x: x,
                    min_z: z,
                    max_x: tile_max_x,
                    max_z: tile_max_z,
                });
            }

            x += tile_size;
        }
        z += tile_size;
    }

    tiles
}

/// Axis-aligned bounding box of a way's nodes: (min_x, max_x, min_z, max_z).
/// None when the way has no nodes.
fn way_aabb(way: &ProcessedWay) -> Option<(i32, i32, i32, i32)> {
    if way.nodes.is_empty() {
        return None;
    }
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;
    for node in &way.nodes {
        min_x = min_x.min(node.x);
        max_x = max_x.max(node.x);
        min_z = min_z.min(node.z);
        max_z = max_z.max(node.z);
    }
    Some((min_x, max_x, min_z, max_z))
}

/// Union AABB over a relation's member ways. None when no member has nodes.
fn relation_aabb(rel: &ProcessedRelation) -> Option<(i32, i32, i32, i32)> {
    let mut acc: Option<(i32, i32, i32, i32)> = None;
    for member in &rel.members {
        if let Some((nx, xx, nz, xz)) = way_aabb(&member.way) {
            acc = Some(match acc {
                None => (nx, xx, nz, xz),
                Some((mn_x, mx_x, mn_z, mx_z)) => {
                    (mn_x.min(nx), mx_x.max(xx), mn_z.min(nz), mx_z.max(xz))
                }
            });
        }
    }
    acc
}

/// AABB-vs-bounds intersection (bounds' max edges are exclusive).
#[inline]
fn aabb_intersects(aabb: (i32, i32, i32, i32), bounds: &TileBounds) -> bool {
    let (min_x, max_x, min_z, max_z) = aabb;
    min_x < bounds.max_x && max_x >= bounds.min_x && min_z < bounds.max_z && max_z >= bounds.min_z
}

/// Inclusive region-cell range (rx0, rx1, rz0, rz1) whose halo-expanded tiles an
/// AABB can intersect. Conservative superset of the matching tiles: for any tile
/// in this range the AABB passes `aabb_intersects(.expanded(halo))`, and any tile
/// outside it cannot. Lets element assignment touch only the relevant region
/// cells instead of scanning every tile.
#[inline]
fn region_range(aabb: (i32, i32, i32, i32), halo: i32) -> (i32, i32, i32, i32) {
    let (min_x, max_x, min_z, max_z) = aabb;
    (
        (min_x - halo) >> 9,
        (max_x + halo) >> 9,
        (min_z - halo) >> 9,
        (max_z + halo) >> 9,
    )
}

/// Check if any of a relation's member ways intersect the given bounds.
fn relation_intersects_bounds(rel: &ProcessedRelation, bounds: &TileBounds) -> bool {
    rel.members
        .iter()
        .any(|member| way_intersects_bounds(&member.way, bounds))
}

/// Check if a way's bounding box intersects with the given bounds.
fn way_intersects_bounds(way: &ProcessedWay, bounds: &TileBounds) -> bool {
    way_aabb(way).is_some_and(|aabb| aabb_intersects(aabb, bounds))
}

/// Check if a way is a linear element (road, railway, barrier, etc.)
fn is_linear_element(way: &ProcessedWay) -> bool {
    way.tags.contains_key("highway")
        || way.tags.contains_key("railway")
        || way.tags.contains_key("barrier")
        || way.tags.contains_key("waterway")
        || way.tags.contains_key("power")
        || way.tags.contains_key("man_made")
        || way.tags.contains_key("aeroway")
}

/// Assign elements to tiles based on spatial relationships.
///
/// Returns a Vec of Vec<usize>, where each inner Vec contains the indices
/// of elements assigned to the corresponding tile.
///
/// Assignment rules:
/// - Point elements (nodes): assigned to the tile containing the point (with halo for trees)
/// - Area elements (buildings, landuse) and relations: assigned to ALL tiles whose
///   editor halo their geometry overlaps (renders large polygons fully and gives
///   per-tile ground generation complete neighbour data across strict boundaries)
/// - Linear elements (roads, railways): assigned to ALL tiles they intersect
pub fn assign_elements_to_tiles(
    elements: &[ProcessedElement],
    tiles: &[TileBounds],
    scale: f64,
) -> Vec<Vec<usize>> {
    let mut tile_elements: Vec<Vec<usize>> = vec![Vec::new(); tiles.len()];
    // Cover the widest rendered linear element (aeroway 40m * scale) so a tile whose
    // strict bounds receive its blocks is assigned it (else per-tile ground overwrites).
    let linear_halo = TILE_EDITOR_HALO.max((MAX_LINEAR_HALF_WIDTH_M * scale).ceil() as i32);

    // Region-coord -> tile index, for O(1) node assignment (tiles are 512-region-aligned).
    let tile_grid: HashMap<(i32, i32), usize> = tiles
        .iter()
        .enumerate()
        .map(|(i, t)| ((t.min_x >> 9, t.min_z >> 9), i))
        .collect();

    for (elem_idx, element) in elements.iter().enumerate() {
        match element {
            ProcessedElement::Node(node) => {
                // A node belongs to the strict tile whose region contains it; the owning
                // tile's editor halo handles canopy overflow. (Strict + non-overlapping, so
                // this matches scanning for the first containing tile, in O(1).)
                if let Some(&tile_idx) = tile_grid.get(&(node.x >> 9, node.z >> 9)) {
                    if tiles[tile_idx].contains(node.x, node.z) {
                        tile_elements[tile_idx].push(elem_idx);
                    }
                }
            }
            ProcessedElement::Way(way) => {
                // Linear elements render to their half-width; areas to the editor halo.
                // Only the region cells the AABB+halo can reach are checked (vs all tiles).
                let Some(aabb) = way_aabb(way) else { continue };
                let halo = if is_linear_element(way) {
                    linear_halo
                } else {
                    TILE_EDITOR_HALO
                };
                let (rx0, rx1, rz0, rz1) = region_range(aabb, halo);
                for rx in rx0..=rx1 {
                    for rz in rz0..=rz1 {
                        if let Some(&tile_idx) = tile_grid.get(&(rx, rz)) {
                            if aabb_intersects(aabb, &tiles[tile_idx].expanded(halo)) {
                                tile_elements[tile_idx].push(elem_idx);
                            }
                        }
                    }
                }
            }
            ProcessedElement::Relation(rel) => {
                // Every tile any member way's AABB+halo overlaps, restricted to the
                // region cells the union AABB+halo can reach.
                let Some(aabb) = relation_aabb(rel) else {
                    continue;
                };
                let (rx0, rx1, rz0, rz1) = region_range(aabb, TILE_EDITOR_HALO);
                for rx in rx0..=rx1 {
                    for rz in rz0..=rz1 {
                        if let Some(&tile_idx) = tile_grid.get(&(rx, rz)) {
                            if relation_intersects_bounds(
                                rel,
                                &tiles[tile_idx].expanded(TILE_EDITOR_HALO),
                            ) {
                                tile_elements[tile_idx].push(elem_idx);
                            }
                        }
                    }
                }
            }
        }
    }

    tile_elements
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedMember, ProcessedMemberRole, ProcessedNode};
    use std::sync::Arc;

    // Deterministic LCG so the fixture is reproducible without rand/Date.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0
        }
        fn coord(&mut self, lo: i32, hi: i32) -> i32 {
            lo + (self.next() % ((hi - lo) as u64 + 1)) as i32
        }
    }

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn way(id: u64, nodes: Vec<ProcessedNode>, linear: bool) -> ProcessedWay {
        let mut tags = HashMap::new();
        if linear {
            tags.insert("highway".to_string(), "residential".to_string());
        } else {
            tags.insert("building".to_string(), "yes".to_string());
        }
        ProcessedWay { id, nodes, tags }
    }

    // Reference O(elements * tiles) assignment matching the original scan exactly.
    fn brute_force(
        elements: &[ProcessedElement],
        tiles: &[TileBounds],
        scale: f64,
    ) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); tiles.len()];
        let linear_halo = TILE_EDITOR_HALO.max((MAX_LINEAR_HALF_WIDTH_M * scale).ceil() as i32);
        for (ei, e) in elements.iter().enumerate() {
            match e {
                ProcessedElement::Node(n) => {
                    for (ti, t) in tiles.iter().enumerate() {
                        if t.contains(n.x, n.z) {
                            out[ti].push(ei);
                            break;
                        }
                    }
                }
                ProcessedElement::Way(w) => {
                    let halo = if is_linear_element(w) {
                        linear_halo
                    } else {
                        TILE_EDITOR_HALO
                    };
                    for (ti, t) in tiles.iter().enumerate() {
                        if way_intersects_bounds(w, &t.expanded(halo)) {
                            out[ti].push(ei);
                        }
                    }
                }
                ProcessedElement::Relation(r) => {
                    for (ti, t) in tiles.iter().enumerate() {
                        if relation_intersects_bounds(r, &t.expanded(TILE_EDITOR_HALO)) {
                            out[ti].push(ei);
                        }
                    }
                }
            }
        }
        out
    }

    // The fast region-range assignment must produce byte-identical output to the
    // exhaustive scan, including per-tile element order, for arbitrary geometry.
    #[test]
    fn assignment_matches_brute_force_scan() {
        let mut rng = Lcg(0x9E3779B97F4A7C15);
        let bbox = XZBBox::rect_from_min_max(-700, -300, 1800, 1500).unwrap();
        let tiles = create_tiles(&bbox, DEFAULT_TILE_SIZE);

        let mut elements: Vec<ProcessedElement> = Vec::new();
        let mut id = 0u64;

        // Scattered nodes, incl. coords on region boundaries and outside the bbox.
        for _ in 0..60 {
            id += 1;
            elements.push(ProcessedElement::Node(node(
                id,
                rng.coord(-900, 2000),
                rng.coord(-500, 1700),
            )));
        }

        // Ways of varied extent: tiny, boundary-hugging, and long multi-tile spans.
        for _ in 0..40 {
            id += 1;
            let n = 2 + (rng.next() % 5) as usize;
            let cx = rng.coord(-700, 1800);
            let cz = rng.coord(-300, 1500);
            let spread = rng.coord(2, 900);
            let nodes: Vec<ProcessedNode> = (0..n)
                .map(|k| {
                    id += 1;
                    node(
                        id,
                        cx + rng.coord(-spread, spread),
                        cz + rng.coord(-spread, spread) + k as i32,
                    )
                })
                .collect();
            let linear = rng.next().is_multiple_of(2);
            elements.push(ProcessedElement::Way(way(id, nodes, linear)));
        }

        // Relations with several member ways spread across the world.
        for _ in 0..12 {
            id += 1;
            let m = 1 + (rng.next() % 3) as usize;
            let members: Vec<ProcessedMember> = (0..m)
                .map(|_| {
                    id += 1;
                    let cx = rng.coord(-700, 1800);
                    let cz = rng.coord(-300, 1500);
                    let spread = rng.coord(2, 600);
                    let nodes: Vec<ProcessedNode> = (0..3)
                        .map(|k| {
                            id += 1;
                            node(
                                id,
                                cx + rng.coord(-spread, spread),
                                cz + rng.coord(-spread, spread) + k,
                            )
                        })
                        .collect();
                    ProcessedMember {
                        role: ProcessedMemberRole::Outer,
                        way: Arc::new(way(id, nodes, false)),
                    }
                })
                .collect();
            elements.push(ProcessedElement::Relation(ProcessedRelation {
                id,
                tags: HashMap::new(),
                members,
            }));
        }

        for &scale in &[1.0_f64, 2.5, 5.0] {
            let fast = assign_elements_to_tiles(&elements, &tiles, scale);
            let brute = brute_force(&elements, &tiles, scale);
            assert_eq!(fast, brute, "mismatch at scale {scale}");
        }
    }
}
