//! Tile subdivision and element assignment for parallel world generation.
//!
//! Divides the world bounding box into a grid of fixed-size tiles (default
//! 512×512 blocks, which aligns with Minecraft region boundaries). Each tile can
//! be processed independently on a separate CPU core.

use crate::coordinate_system::cartesian::XZBBox;
#[cfg(test)]
use crate::osm_parser::ProcessedRelation;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
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
///
/// The grid is aligned to multiples of `tile_size` measured from the world origin,
/// so every tile corner is a multiple of `tile_size` and a tile is addressable from
/// (grid origin, tile size) alone. With the default 512 this is exactly a Minecraft
/// region boundary and the emitted tiles are unchanged. Tiles at the edge of the
/// grid are clamped to the aligned extent and may be smaller than the full tile size.
///
/// Panics if `tile_size` is not positive.
pub fn create_tiles(xzbbox: &XZBBox, tile_size: i32) -> Vec<TileBounds> {
    assert!(tile_size > 0, "tile_size must be positive, got {tile_size}");

    let mut tiles = Vec::new();

    // Align the tile grid to multiples of tile_size (floor for min, ceil for max).
    // For the default 512 these are exactly the old `>> 9 << 9` region-aligned
    // expressions, so the production grid is bit-for-bit what it always was.
    let aligned_min_x = xzbbox.min_x().div_euclid(tile_size) * tile_size;
    let aligned_min_z = xzbbox.min_z().div_euclid(tile_size) * tile_size;
    let aligned_max_x = (xzbbox.max_x().div_euclid(tile_size) + 1) * tile_size; // exclusive
    let aligned_max_z = (xzbbox.max_z().div_euclid(tile_size) + 1) * tile_size;

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

/// AABB-vs-bounds intersection (bounds' max edges are exclusive).
#[inline]
fn aabb_intersects(aabb: (i32, i32, i32, i32), bounds: &TileBounds) -> bool {
    let (min_x, max_x, min_z, max_z) = aabb;
    min_x < bounds.max_x && max_x >= bounds.min_x && min_z < bounds.max_z && max_z >= bounds.min_z
}

/// The uniform grid a tile list sits on: the corner tiles are measured from, the
/// per-axis spacing between neighbouring tiles, and the cell -> tile index lookup.
///
/// Derived from the tiles themselves so every tile size works. Keying on fixed
/// 512-block region cells only ever worked for `tile_size == 512`: smaller tiles
/// collided on one key (all but one tile became unreachable and rendered bare
/// ground) and larger tiles had no entry for the cells past their min corner.
struct TileGrid {
    origin_x: i32,
    origin_z: i32,
    step_x: i32,
    step_z: i32,
    /// Grid cell (x, z) -> index into the tile slice.
    cells: HashMap<(i32, i32), usize>,
}

impl TileGrid {
    /// Build the grid of a tile list produced by [`create_tiles`].
    /// None when the list is empty.
    fn new(tiles: &[TileBounds]) -> Option<TileGrid> {
        let origin_x = tiles.iter().map(|t| t.min_x).min()?;
        let origin_z = tiles.iter().map(|t| t.min_z).min()?;
        // Edge tiles can be clamped short of the tile size, so the spacing is the
        // widest tile; every tile then still lies wholly inside its own cell, which
        // is all the range scan below relies on.
        let step_x = tiles.iter().map(|t| t.max_x - t.min_x).max()?.max(1);
        let step_z = tiles.iter().map(|t| t.max_z - t.min_z).max()?.max(1);

        let mut grid = TileGrid {
            origin_x,
            origin_z,
            step_x,
            step_z,
            cells: HashMap::with_capacity(tiles.len()),
        };
        for (i, tile) in tiles.iter().enumerate() {
            let cell = grid.cell_of(tile.min_x, tile.min_z);
            grid.cells.insert(cell, i);
        }
        Some(grid)
    }

    /// Grid cell containing a block coordinate.
    #[inline]
    fn cell_of(&self, x: i32, z: i32) -> (i32, i32) {
        (
            x.saturating_sub(self.origin_x).div_euclid(self.step_x),
            z.saturating_sub(self.origin_z).div_euclid(self.step_z),
        )
    }

    /// Index of the tile occupying a grid cell, if a tile was created there.
    #[inline]
    fn tile_at(&self, cell: (i32, i32)) -> Option<usize> {
        self.cells.get(&cell).copied()
    }

    /// Inclusive cell range (cx0, cx1, cz0, cz1) whose halo-expanded tiles an AABB
    /// can intersect. Conservative superset of the matching tiles: for any tile in
    /// this range the AABB may pass `aabb_intersects(.expanded(halo))`, and any tile
    /// outside it cannot. Lets element assignment touch only the relevant cells
    /// instead of scanning every tile.
    #[inline]
    fn cell_range(&self, aabb: (i32, i32, i32, i32), halo: i32) -> (i32, i32, i32, i32) {
        let (min_x, max_x, min_z, max_z) = aabb;
        let (cx0, cz0) = self.cell_of(min_x.saturating_sub(halo), min_z.saturating_sub(halo));
        let (cx1, cz1) = self.cell_of(max_x.saturating_add(halo), max_z.saturating_add(halo));
        (cx0, cx1, cz0, cz1)
    }
}

/// Check if a way's bounding box intersects with the given bounds.
#[cfg(test)]
fn way_intersects_bounds(way: &ProcessedWay, bounds: &TileBounds) -> bool {
    way_aabb(way).is_some_and(|aabb| aabb_intersects(aabb, bounds))
}

/// Check if any of a relation's member ways intersect the given bounds.
#[cfg(test)]
fn relation_intersects_bounds(rel: &ProcessedRelation, bounds: &TileBounds) -> bool {
    rel.members
        .iter()
        .any(|member| way_intersects_bounds(&member.way, bounds))
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
///
/// Works for any tile size: the lookup is keyed on the tile grid the `tiles` list
/// itself describes, not on fixed 512-block region cells.
pub fn assign_elements_to_tiles(
    elements: &[ProcessedElement],
    tiles: &[TileBounds],
    scale: f64,
) -> Vec<Vec<usize>> {
    let mut tile_elements: Vec<Vec<usize>> = vec![Vec::new(); tiles.len()];
    let Some(grid) = TileGrid::new(tiles) else {
        return tile_elements;
    };
    // Cover the widest rendered linear element (aeroway 40m * scale) so a tile whose
    // strict bounds receive its blocks is assigned it (else per-tile ground overwrites).
    let linear_halo = TILE_EDITOR_HALO.max((MAX_LINEAR_HALF_WIDTH_M * scale).ceil() as i32);

    for (elem_idx, element) in elements.iter().enumerate() {
        match element {
            ProcessedElement::Node(node) => {
                // Helipad node discs must be authoritative in every tile they touch.
                if node.tags.get("aeroway").map(String::as_str) == Some("helipad") {
                    let radius_m = crate::element_processing::highways::HELIPAD_NODE_RADIUS_M;
                    let reach = ((radius_m * scale).round() as i32).max(4) + 12;
                    let aabb = (
                        node.x - reach,
                        node.x + reach,
                        node.z - reach,
                        node.z + reach,
                    );
                    let (cx0, cx1, cz0, cz1) = grid.cell_range(aabb, 0);
                    for cx in cx0..=cx1 {
                        for cz in cz0..=cz1 {
                            if let Some(tile_idx) = grid.tile_at((cx, cz)) {
                                if aabb_intersects(aabb, &tiles[tile_idx]) {
                                    tile_elements[tile_idx].push(elem_idx);
                                }
                            }
                        }
                    }
                    continue;
                }
                // A node belongs to the strict tile whose grid cell contains it; the
                // owning tile's editor halo handles canopy overflow. (Strict +
                // non-overlapping, so this matches scanning for the first containing
                // tile, in O(1).)
                if let Some(tile_idx) = grid.tile_at(grid.cell_of(node.x, node.z)) {
                    if tiles[tile_idx].contains(node.x, node.z) {
                        tile_elements[tile_idx].push(elem_idx);
                    }
                }
            }
            ProcessedElement::Way(way) => {
                // Linear elements render to their half-width; areas to the editor halo.
                // Only the grid cells the AABB+halo can reach are checked (vs all tiles).
                let Some(aabb) = way_aabb(way) else { continue };
                let halo = if is_linear_element(way) {
                    linear_halo
                } else {
                    TILE_EDITOR_HALO
                };
                let (cx0, cx1, cz0, cz1) = grid.cell_range(aabb, halo);
                for cx in cx0..=cx1 {
                    for cz in cz0..=cz1 {
                        if let Some(tile_idx) = grid.tile_at((cx, cz)) {
                            if aabb_intersects(aabb, &tiles[tile_idx].expanded(halo)) {
                                tile_elements[tile_idx].push(elem_idx);
                            }
                        }
                    }
                }
            }
            ProcessedElement::Relation(rel) => {
                // Every tile any member way's AABB+halo overlaps, restricted to the
                // grid cells the union AABB+halo can reach.
                // Cache member AABBs once: relation_intersects_bounds used to walk
                // every member's nodes again for every candidate tile.
                let member_aabbs: Vec<(i32, i32, i32, i32)> = rel
                    .members
                    .iter()
                    .filter_map(|member| way_aabb(&member.way))
                    .collect();
                let Some(aabb) = member_aabbs.iter().copied().reduce(
                    |(mn_x, mx_x, mn_z, mx_z), (nx, xx, nz, xz)| {
                        (mn_x.min(nx), mx_x.max(xx), mn_z.min(nz), mx_z.max(xz))
                    },
                ) else {
                    continue;
                };
                let (cx0, cx1, cz0, cz1) = grid.cell_range(aabb, TILE_EDITOR_HALO);
                for cx in cx0..=cx1 {
                    for cz in cz0..=cz1 {
                        if let Some(tile_idx) = grid.tile_at((cx, cz)) {
                            let expanded = tiles[tile_idx].expanded(TILE_EDITOR_HALO);
                            if member_aabbs
                                .iter()
                                .copied()
                                .any(|member_aabb| aabb_intersects(member_aabb, &expanded))
                            {
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

    // Tile sizes the grid must handle: below, at and above the 512 region size.
    const TILE_SIZES: [i32; 4] = [128, 256, 512, 1024];

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

    fn test_bbox() -> XZBBox {
        XZBBox::rect_from_min_max(-700, -300, 1800, 1500).unwrap()
    }

    // Mixed fixture: scattered nodes (incl. on tile boundaries and outside the bbox),
    // ways from tiny to multi-tile spanning, and multi-member relations.
    fn fixture_elements() -> Vec<ProcessedElement> {
        let mut rng = Lcg(0x9E3779B97F4A7C15);
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

        elements
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

    // The pre-parameterisation tile grid, verbatim: floors/ceils to 512-block region
    // cells regardless of tile_size. The oracle for "512 output must not change".
    fn region_aligned_create_tiles(xzbbox: &XZBBox, tile_size: i32) -> Vec<TileBounds> {
        let mut tiles = Vec::new();
        let aligned_min_x = (xzbbox.min_x() >> 9) << 9;
        let aligned_min_z = (xzbbox.min_z() >> 9) << 9;
        let aligned_max_x = ((xzbbox.max_x() + 512) >> 9) << 9;
        let aligned_max_z = ((xzbbox.max_z() + 512) >> 9) << 9;

        let mut z = aligned_min_z;
        while z < aligned_max_z {
            let mut x = aligned_min_x;
            while x < aligned_max_x {
                let tile_max_x = (x + tile_size).min(aligned_max_x);
                let tile_max_z = (z + tile_size).min(aligned_max_z);
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

    fn tile_tuples(tiles: &[TileBounds]) -> Vec<(i32, i32, i32, i32)> {
        tiles
            .iter()
            .map(|t| (t.min_x, t.min_z, t.max_x, t.max_z))
            .collect()
    }

    // Every node coordinate of an element, for the "inside the world bbox" check.
    fn element_coords(element: &ProcessedElement) -> Vec<(i32, i32)> {
        match element {
            ProcessedElement::Node(n) => vec![(n.x, n.z)],
            ProcessedElement::Way(w) => w.nodes.iter().map(|n| (n.x, n.z)).collect(),
            ProcessedElement::Relation(r) => r
                .members
                .iter()
                .flat_map(|m| m.way.nodes.iter().map(|n| (n.x, n.z)))
                .collect(),
        }
    }

    // The fast grid assignment must produce byte-identical output to the exhaustive
    // scan, including per-tile element order, for arbitrary geometry — at every tile
    // size, not just 512. Sizes below 512 used to collide several tiles on one region
    // key (all but one silently unassigned); sizes above 512 left the cells past a
    // tile's min corner unmapped.
    #[test]
    fn assignment_matches_brute_force_scan() {
        let bbox = test_bbox();
        let elements = fixture_elements();

        for tile_size in TILE_SIZES {
            let tiles = create_tiles(&bbox, tile_size);
            assert!(!tiles.is_empty(), "no tiles at tile size {tile_size}");
            for &scale in &[1.0_f64, 2.5, 5.0] {
                let fast = assign_elements_to_tiles(&elements, &tiles, scale);
                let brute = brute_force(&elements, &tiles, scale);
                assert_eq!(
                    fast, brute,
                    "mismatch at tile size {tile_size}, scale {scale}"
                );
            }
        }
    }

    // Production passes DEFAULT_TILE_SIZE, so the 512 grid must be exactly the
    // region-aligned grid emitted before tile size became a real parameter.
    #[test]
    fn default_tile_size_grid_is_unchanged() {
        let bboxes = [
            (-700, -300, 1800, 1500),
            (0, 0, 511, 511),
            (0, 0, 512, 512),
            (-1, -1, 1, 1),
            (-2048, -1025, -1500, -600),
            (100, 200, 5000, 4000),
        ];
        for (min_x, min_z, max_x, max_z) in bboxes {
            let bbox = XZBBox::rect_from_min_max(min_x, min_z, max_x, max_z).unwrap();
            let now = create_tiles(&bbox, DEFAULT_TILE_SIZE);
            let before = region_aligned_create_tiles(&bbox, DEFAULT_TILE_SIZE);
            assert_eq!(
                tile_tuples(&now),
                tile_tuples(&before),
                "512 tile grid changed for bbox ({min_x}, {min_z}, {max_x}, {max_z})"
            );
        }
    }

    // The grid is aligned to the requested tile size and covers the whole bbox.
    #[test]
    fn grid_is_aligned_to_tile_size() {
        let bbox = test_bbox();
        for tile_size in TILE_SIZES {
            let tiles = create_tiles(&bbox, tile_size);
            for t in &tiles {
                assert_eq!(
                    t.min_x.rem_euclid(tile_size),
                    0,
                    "tile min_x {} not aligned to {tile_size}",
                    t.min_x
                );
                assert_eq!(
                    t.min_z.rem_euclid(tile_size),
                    0,
                    "tile min_z {} not aligned to {tile_size}",
                    t.min_z
                );
                assert!(t.max_x > t.min_x && t.max_z > t.min_z);
            }
            // Every corner of the bbox lands in exactly one tile.
            for &(x, z) in &[
                (bbox.min_x(), bbox.min_z()),
                (bbox.max_x(), bbox.min_z()),
                (bbox.min_x(), bbox.max_z()),
                (bbox.max_x(), bbox.max_z()),
            ] {
                let hits = tiles.iter().filter(|t| t.contains(x, z)).count();
                assert_eq!(
                    hits, 1,
                    "({x}, {z}) hit {hits} tiles at tile size {tile_size}"
                );
            }
        }
    }

    // The bug this file had: with a mis-keyed lookup whole tiles received no elements
    // at all. Nothing inside the world bbox may end up assigned to zero tiles.
    #[test]
    fn in_bbox_elements_are_never_orphaned() {
        let bbox = test_bbox();
        let elements = fixture_elements();

        for tile_size in TILE_SIZES {
            let tiles = create_tiles(&bbox, tile_size);
            let assignments = assign_elements_to_tiles(&elements, &tiles, 1.0);

            let mut assigned = vec![false; elements.len()];
            for tile in &assignments {
                for &elem_idx in tile {
                    assigned[elem_idx] = true;
                }
            }

            for (elem_idx, element) in elements.iter().enumerate() {
                let coords = element_coords(element);
                let inside = !coords.is_empty()
                    && coords.iter().all(|&(x, z)| {
                        x >= bbox.min_x()
                            && x <= bbox.max_x()
                            && z >= bbox.min_z()
                            && z <= bbox.max_z()
                    });
                if inside {
                    assert!(
                        assigned[elem_idx],
                        "element {elem_idx} lies inside the world bbox but was assigned to \
                         no tile at tile size {tile_size}"
                    );
                }
            }
        }
    }
}
