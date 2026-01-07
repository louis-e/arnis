//! Region-by-region processing utilities for memory-efficient world generation.
//!
//! This module provides utilities to process the world one region at a time,
//! significantly reducing peak memory usage for large worlds.

use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::ProcessedElement;

/// Size of a Minecraft region in blocks (32 chunks × 16 blocks = 512)
pub const REGION_SIZE: i32 = 512;

/// Represents a region's bounds in world coordinates
#[derive(Clone, Debug)]
pub struct RegionBounds {
    pub region_x: i32,
    pub region_z: i32,
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
}

impl RegionBounds {
    /// Create region bounds from region coordinates
    pub fn from_region_coords(region_x: i32, region_z: i32) -> Self {
        Self {
            region_x,
            region_z,
            min_x: region_x * REGION_SIZE,
            max_x: region_x * REGION_SIZE + REGION_SIZE - 1,
            min_z: region_z * REGION_SIZE,
            max_z: region_z * REGION_SIZE + REGION_SIZE - 1,
        }
    }

    /// Check if a point is within this region
    #[inline]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }

    /// Check if a bounding box intersects with this region
    #[inline]
    pub fn intersects_bbox(&self, min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> bool {
        self.min_x <= max_x && self.max_x >= min_x && self.min_z <= max_z && self.max_z >= min_z
    }
}

/// Calculate all region bounds that intersect with the world bounding box
pub fn calculate_region_bounds(xzbbox: &XZBBox) -> Vec<RegionBounds> {
    let min_region_x = xzbbox.min_x().div_euclid(REGION_SIZE);
    let max_region_x = xzbbox.max_x().div_euclid(REGION_SIZE);
    let min_region_z = xzbbox.min_z().div_euclid(REGION_SIZE);
    let max_region_z = xzbbox.max_z().div_euclid(REGION_SIZE);

    let mut regions = Vec::new();
    for region_x in min_region_x..=max_region_x {
        for region_z in min_region_z..=max_region_z {
            regions.push(RegionBounds::from_region_coords(region_x, region_z));
        }
    }
    regions
}

/// Get the bounding box of an element (min_x, max_x, min_z, max_z)
pub fn element_bounds(element: &ProcessedElement) -> Option<(i32, i32, i32, i32)> {
    match element {
        ProcessedElement::Node(node) => Some((node.x, node.x, node.z, node.z)),
        ProcessedElement::Way(way) => {
            if way.nodes.is_empty() {
                return None;
            }
            let min_x = way.nodes.iter().map(|n| n.x).min().unwrap();
            let max_x = way.nodes.iter().map(|n| n.x).max().unwrap();
            let min_z = way.nodes.iter().map(|n| n.z).min().unwrap();
            let max_z = way.nodes.iter().map(|n| n.z).max().unwrap();
            Some((min_x, max_x, min_z, max_z))
        }
        ProcessedElement::Relation(rel) => {
            if rel.members.is_empty() {
                return None;
            }
            let mut min_x = i32::MAX;
            let mut max_x = i32::MIN;
            let mut min_z = i32::MAX;
            let mut max_z = i32::MIN;

            for member in &rel.members {
                for node in &member.way.nodes {
                    min_x = min_x.min(node.x);
                    max_x = max_x.max(node.x);
                    min_z = min_z.min(node.z);
                    max_z = max_z.max(node.z);
                }
            }

            if min_x == i32::MAX {
                return None;
            }
            Some((min_x, max_x, min_z, max_z))
        }
    }
}

/// Check if an element intersects with a region
pub fn element_intersects_region(element: &ProcessedElement, region: &RegionBounds) -> bool {
    if let Some((min_x, max_x, min_z, max_z)) = element_bounds(element) {
        region.intersects_bbox(min_x, max_x, min_z, max_z)
    } else {
        false
    }
}

/// Create a region-scoped XZBBox that is the intersection of the world bbox and region bounds
pub fn create_region_bbox(world_bbox: &XZBBox, region: &RegionBounds) -> Option<XZBBox> {
    // Calculate intersection
    let min_x = world_bbox.min_x().max(region.min_x);
    let max_x = world_bbox.max_x().min(region.max_x);
    let min_z = world_bbox.min_z().max(region.min_z);
    let max_z = world_bbox.max_z().min(region.max_z);

    // Check if intersection is valid
    if min_x > max_x || min_z > max_z {
        return None;
    }

    // Create bbox for this region's portion
    XZBBox::rect_from_xz_lengths((max_x - min_x) as f64, (max_z - min_z) as f64)
        .ok()
        .map(|mut bbox| {
            // Offset to correct position
            use crate::coordinate_system::cartesian::XZVector;
            bbox += XZVector { dx: min_x, dz: min_z };
            bbox
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_region_bounds() {
        let region = RegionBounds::from_region_coords(0, 0);
        assert_eq!(region.min_x, 0);
        assert_eq!(region.max_x, 511);
        assert_eq!(region.min_z, 0);
        assert_eq!(region.max_z, 511);

        let region = RegionBounds::from_region_coords(1, -1);
        assert_eq!(region.min_x, 512);
        assert_eq!(region.max_x, 1023);
        assert_eq!(region.min_z, -512);
        assert_eq!(region.max_z, -1);
    }

    #[test]
    fn test_region_contains() {
        let region = RegionBounds::from_region_coords(0, 0);
        assert!(region.contains(0, 0));
        assert!(region.contains(511, 511));
        assert!(region.contains(256, 256));
        assert!(!region.contains(512, 0));
        assert!(!region.contains(-1, 0));
    }

    #[test]
    fn test_region_intersects() {
        let region = RegionBounds::from_region_coords(0, 0);

        // Fully inside
        assert!(region.intersects_bbox(100, 200, 100, 200));

        // Partially overlapping
        assert!(region.intersects_bbox(-100, 100, -100, 100));
        assert!(region.intersects_bbox(400, 600, 400, 600));

        // Touching edge
        assert!(region.intersects_bbox(511, 600, 0, 100));

        // Fully outside
        assert!(!region.intersects_bbox(600, 700, 0, 100));
        assert!(!region.intersects_bbox(-200, -100, 0, 100));
    }
}
