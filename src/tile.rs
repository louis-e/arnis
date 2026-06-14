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

/// Check if any of a relation's member ways intersect the given bounds.
fn relation_intersects_bounds(rel: &ProcessedRelation, bounds: &TileBounds) -> bool {
    rel.members
        .iter()
        .any(|member| way_intersects_bounds(&member.way, bounds))
}

/// Check if a way's bounding box intersects with the given bounds.
fn way_intersects_bounds(way: &ProcessedWay, bounds: &TileBounds) -> bool {
    if way.nodes.is_empty() {
        return false;
    }
    let mut way_min_x = i32::MAX;
    let mut way_max_x = i32::MIN;
    let mut way_min_z = i32::MAX;
    let mut way_max_z = i32::MIN;
    for node in &way.nodes {
        way_min_x = way_min_x.min(node.x);
        way_max_x = way_max_x.max(node.x);
        way_min_z = way_min_z.min(node.z);
        way_max_z = way_max_z.max(node.z);
    }
    // AABB intersection test
    way_min_x < bounds.max_x
        && way_max_x >= bounds.min_x
        && way_min_z < bounds.max_z
        && way_max_z >= bounds.min_z
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
                if is_linear_element(way) {
                    // Linear: every tile within the rendered half-width of the centerline.
                    for (tile_idx, tile) in tiles.iter().enumerate() {
                        if way_intersects_bounds(way, &tile.expanded(linear_halo)) {
                            tile_elements[tile_idx].push(elem_idx);
                        }
                    }
                } else {
                    // Area: every tile its AABB+halo overlaps, so large polygons render
                    // fully and per-tile ground sees complete neighbour data.
                    for (tile_idx, tile) in tiles.iter().enumerate() {
                        if way_intersects_bounds(way, &tile.expanded(TILE_EDITOR_HALO)) {
                            tile_elements[tile_idx].push(elem_idx);
                        }
                    }
                }
            }
            ProcessedElement::Relation(rel) => {
                // Relations: every tile any member way's AABB+halo overlaps.
                for (tile_idx, tile) in tiles.iter().enumerate() {
                    if relation_intersects_bounds(rel, &tile.expanded(TILE_EDITOR_HALO)) {
                        tile_elements[tile_idx].push(elem_idx);
                    }
                }
            }
        }
    }

    tile_elements
}
