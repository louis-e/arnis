//! Parallel region processing for improved memory efficiency and CPU utilization.
//!
//! This module splits the world generation into processing units (1 Minecraft region each),
//! processes them in parallel, and flushes each region to disk immediately after completion.
//!
//! Key benefits:
//! - Memory usage reduced by ~90% (only active regions in memory)
//! - Multi-core CPU utilization
//! - Consistent results via deterministic RNG

use crate::coordinate_system::cartesian::xzbbox::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::element_processing::highways::HighwayConnectivityMap;

/// Size of a Minecraft region in blocks (32 chunks × 16 blocks per chunk)
pub const REGION_BLOCKS: i32 = 512;

/// A processing unit representing a single Minecraft region to be generated.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProcessingUnit {
    /// Region X coordinate (in region space, not block space)
    pub region_x: i32,
    /// Region Z coordinate (in region space, not block space)
    pub region_z: i32,

    /// Minecraft coordinate bounds for this unit (512×512 blocks)
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,

    /// Expanded bounds for element fetching (includes buffer for boundary elements)
    pub fetch_min_x: i32,
    pub fetch_max_x: i32,
    pub fetch_min_z: i32,
    pub fetch_max_z: i32,
}

impl ProcessingUnit {
    /// Creates a new processing unit for a specific region.
    #[allow(dead_code)]
    pub fn new(region_x: i32, region_z: i32, global_bbox: &XZBBox, buffer_blocks: i32) -> Self {
        Self::new_batched(region_x, region_z, region_x, region_z, global_bbox, buffer_blocks)
    }
    
    /// Creates a processing unit spanning multiple regions (for batching).
    pub fn new_batched(
        start_region_x: i32, start_region_z: i32,
        end_region_x: i32, end_region_z: i32,
        global_bbox: &XZBBox, buffer_blocks: i32
    ) -> Self {
        // Calculate block bounds for these regions
        let min_x = start_region_x * REGION_BLOCKS;
        let max_x = (end_region_x + 1) * REGION_BLOCKS - 1;
        let min_z = start_region_z * REGION_BLOCKS;
        let max_z = (end_region_z + 1) * REGION_BLOCKS - 1;

        // Add buffer for fetch bounds, clamped to global bbox
        let fetch_min_x = (min_x - buffer_blocks).max(global_bbox.min_x());
        let fetch_max_x = (max_x + buffer_blocks).min(global_bbox.max_x());
        let fetch_min_z = (min_z - buffer_blocks).max(global_bbox.min_z());
        let fetch_max_z = (max_z + buffer_blocks).min(global_bbox.max_z());

        Self {
            region_x: start_region_x,
            region_z: start_region_z,
            min_x,
            max_x,
            min_z,
            max_z,
            fetch_min_x,
            fetch_max_x,
            fetch_min_z,
            fetch_max_z,
        }
    }

    /// Returns the XZBBox for this unit's actual processing bounds (not fetch bounds).
    pub fn bbox(&self) -> XZBBox {
        XZBBox::Rect(
            XZBBoxRect::new(
                XZPoint::new(self.min_x, self.min_z),
                XZPoint::new(self.max_x, self.max_z),
            )
            .expect("Invalid unit bbox bounds"),
        )
    }

    /// Returns the XZBBox for this unit's fetch bounds (includes buffer).
    #[allow(dead_code)]
    pub fn fetch_bbox(&self) -> XZBBox {
        XZBBox::Rect(
            XZBBoxRect::new(
                XZPoint::new(self.fetch_min_x, self.fetch_min_z),
                XZPoint::new(self.fetch_max_x, self.fetch_max_z),
            )
            .expect("Invalid unit fetch bbox bounds"),
        )
    }

    /// Checks if a point is within this unit's fetch bounds.
    #[inline]
    #[allow(dead_code)]
    pub fn contains_fetch(&self, x: i32, z: i32) -> bool {
        x >= self.fetch_min_x
            && x <= self.fetch_max_x
            && z >= self.fetch_min_z
            && z <= self.fetch_max_z
    }

    /// Checks if an element's bounding box intersects with this unit's fetch bounds.
    pub fn intersects_element(&self, element: &ProcessedElement) -> bool {
        let (min_x, max_x, min_z, max_z) = element_bbox(element);

        // Check for intersection
        !(max_x < self.fetch_min_x
            || min_x > self.fetch_max_x
            || max_z < self.fetch_min_z
            || min_z > self.fetch_max_z)
    }
}

/// Computes the bounding box of an element.
fn element_bbox(element: &ProcessedElement) -> (i32, i32, i32, i32) {
    match element {
        ProcessedElement::Node(node) => (node.x, node.x, node.z, node.z),
        ProcessedElement::Way(way) => way_bbox(&way.nodes),
        ProcessedElement::Relation(rel) => {
            let mut min_x = i32::MAX;
            let mut max_x = i32::MIN;
            let mut min_z = i32::MAX;
            let mut max_z = i32::MIN;

            for member in &rel.members {
                let (mx, mxx, mz, mxz) = way_bbox(&member.way.nodes);
                min_x = min_x.min(mx);
                max_x = max_x.max(mxx);
                min_z = min_z.min(mz);
                max_z = max_z.max(mxz);
            }

            (min_x, max_x, min_z, max_z)
        }
    }
}

/// Computes the bounding box of a way's nodes.
fn way_bbox(nodes: &[ProcessedNode]) -> (i32, i32, i32, i32) {
    if nodes.is_empty() {
        return (0, 0, 0, 0);
    }

    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;

    for node in nodes {
        min_x = min_x.min(node.x);
        max_x = max_x.max(node.x);
        min_z = min_z.min(node.z);
        max_z = max_z.max(node.z);
    }

    (min_x, max_x, min_z, max_z)
}

/// Computes all processing units for a given world bounding box.
/// With batch_size=1, creates one unit per region.
/// With batch_size=2, creates one unit per 2x2 = 4 regions, etc.
pub fn compute_processing_units(
    global_bbox: &XZBBox,
    buffer_blocks: i32,
    batch_size: usize,
) -> Vec<ProcessingUnit> {
    // Calculate which regions are covered by the bbox
    let min_region_x = global_bbox.min_x() >> 9; // divide by 512
    let max_region_x = global_bbox.max_x() >> 9;
    let min_region_z = global_bbox.min_z() >> 9;
    let max_region_z = global_bbox.max_z() >> 9;

    let mut units = Vec::new();
    
    // Batch size determines how many regions are grouped together
    // batch_size=1 -> 1 region per unit
    // batch_size=2 -> 2x2=4 regions per unit
    let batch = batch_size.max(1) as i32;

    // Create units grouped by batch_size
    let mut rx = min_region_x;
    while rx <= max_region_x {
        let mut rz = min_region_z;
        while rz <= max_region_z {
            // Create a unit spanning batch_size regions in each direction
            units.push(ProcessingUnit::new_batched(
                rx, rz, 
                (rx + batch - 1).min(max_region_x),
                (rz + batch - 1).min(max_region_z),
                global_bbox, 
                buffer_blocks
            ));
            rz += batch;
        }
        rx += batch;
    }

    units
}

/// Distributes elements to processing units based on spatial intersection.
///
/// Each element is assigned to all units whose fetch bounds it intersects.
/// This ensures elements at boundaries are processed by all relevant units.
#[allow(dead_code)]
pub fn distribute_elements_to_units<'a>(
    elements: &'a [ProcessedElement],
    units: &[ProcessingUnit],
) -> Vec<Vec<&'a ProcessedElement>> {
    let mut unit_elements: Vec<Vec<&ProcessedElement>> = vec![Vec::new(); units.len()];

    for element in elements {
        for (i, unit) in units.iter().enumerate() {
            if unit.intersects_element(element) {
                unit_elements[i].push(element);
            }
        }
    }

    unit_elements
}

/// Distributes elements to processing units, returning indices instead of references.
///
/// This is useful when elements need to be shared across threads via Arc.
pub fn distribute_elements_to_units_indices(
    elements: &[ProcessedElement],
    units: &[ProcessingUnit],
) -> Vec<Vec<usize>> {
    let mut unit_indices: Vec<Vec<usize>> = vec![Vec::new(); units.len()];

    for (idx, element) in elements.iter().enumerate() {
        for (i, unit) in units.iter().enumerate() {
            if unit.intersects_element(element) {
                unit_indices[i].push(idx);
            }
        }
    }

    unit_indices
}

/// Global shared data that must be computed once and shared across all processing units.
#[allow(dead_code)]
pub struct GlobalSharedData {
    /// Ground/elevation data (must be consistent across boundaries)
    pub ground: Arc<Ground>,
    /// Building footprints bitmap (prevents trees inside buildings at boundaries)
    pub building_footprints: Arc<BuildingFootprintBitmap>,
    /// Highway connectivity map (for intersection detection)
    pub highway_connectivity: Arc<HighwayConnectivityMap>,
}

/// Statistics from parallel processing.
#[derive(Default)]
#[allow(dead_code)]
pub struct ProcessingStats {
    pub total_units: u64,
    pub completed_units: AtomicU64,
    pub total_elements: u64,
}

impl ProcessingStats {
    pub fn new(total_units: usize, total_elements: usize) -> Self {
        Self {
            total_units: total_units as u64,
            completed_units: AtomicU64::new(0),
            total_elements: total_elements as u64,
        }
    }

    pub fn increment_completed(&self) -> u64 {
        self.completed_units.fetch_add(1, Ordering::SeqCst) + 1
    }

    #[allow(dead_code)]
    pub fn progress_percentage(&self) -> f64 {
        let completed = self.completed_units.load(Ordering::SeqCst);
        if self.total_units == 0 {
            100.0
        } else {
            (completed as f64 / self.total_units as f64) * 100.0
        }
    }
}

/// Clips an element to a unit's actual processing bounds (not fetch bounds).
///
/// Returns Some(clipped_element) if the element has any part within the unit's bounds,
/// or None if the element is completely outside.
#[allow(dead_code)]
pub fn clip_element_to_unit(
    element: &ProcessedElement,
    unit: &ProcessingUnit,
) -> Option<ProcessedElement> {
    match element {
        ProcessedElement::Node(node) => {
            // Nodes are either fully inside or outside
            if node.x >= unit.min_x
                && node.x <= unit.max_x
                && node.z >= unit.min_z
                && node.z <= unit.max_z
            {
                Some(element.clone())
            } else {
                None
            }
        }
        ProcessedElement::Way(way) => {
            // For ways, we keep the full way but let the WorldEditor handle bounds checking
            // This ensures deterministic RNG produces the same results regardless of clipping
            // The WorldEditor.set_block() already checks bounds via xzbbox.contains()
            let (min_x, max_x, min_z, max_z) = way_bbox(&way.nodes);

            // Check if way intersects unit bounds
            if max_x < unit.min_x
                || min_x > unit.max_x
                || max_z < unit.min_z
                || min_z > unit.max_z
            {
                return None;
            }

            Some(element.clone())
        }
        ProcessedElement::Relation(_rel) => {
            // For relations, similar approach - keep full relation for consistent processing
            let (min_x, max_x, min_z, max_z) = element_bbox(element);

            // Check if relation intersects unit bounds
            if max_x < unit.min_x
                || min_x > unit.max_x
                || max_z < unit.min_z
                || min_z > unit.max_z
            {
                return None;
            }

            Some(element.clone())
        }
    }
}

/// Clips a collection of elements to a unit's bounds.
#[allow(dead_code)]
pub fn clip_elements_to_unit(
    elements: &[&ProcessedElement],
    unit: &ProcessingUnit,
) -> Vec<ProcessedElement> {
    elements
        .iter()
        .filter_map(|e| clip_element_to_unit(e, unit))
        .collect()
}

/// Calculates the number of parallel threads to use.
pub fn calculate_parallel_threads(requested: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    if requested == 0 {
        // Default: use all but one core
        available.saturating_sub(1).max(1)
    } else {
        // Use requested amount, capped at available
        requested.min(available).max(1)
    }
}

/// Configuration for parallel processing
pub struct ParallelConfig {
    /// Number of threads to use (0 = auto, uses available - 1)
    pub num_threads: usize,
    /// Buffer in blocks around each unit for element fetching
    pub buffer_blocks: i32,
    /// Whether to use parallel processing (false = sequential for debugging)
    pub enabled: bool,
    /// Number of regions to batch per unit (1 = single region, 2 = 2x2 = 4 regions)
    pub region_batch_size: usize,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            num_threads: 0, // Auto-detect
            buffer_blocks: 64, // Buffer for boundary elements
            enabled: true,
            region_batch_size: 2, // 2x2=4 regions per unit - optimal balance
        }
    }
}

impl ParallelConfig {
    pub fn sequential() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::xzbbox::rectangle::XZBBoxRect;

    fn make_test_bbox(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> XZBBox {
        XZBBox::Rect(
            XZBBoxRect::new(XZPoint::new(min_x, min_z), XZPoint::new(max_x, max_z)).unwrap(),
        )
    }

    #[test]
    fn test_processing_unit_creation() {
        let global_bbox = make_test_bbox(0, 1023, 0, 1023);
        let unit = ProcessingUnit::new(0, 0, &global_bbox, 64);

        assert_eq!(unit.region_x, 0);
        assert_eq!(unit.region_z, 0);
        assert_eq!(unit.min_x, 0);
        assert_eq!(unit.max_x, 511);
        assert_eq!(unit.min_z, 0);
        assert_eq!(unit.max_z, 511);
        // Fetch bounds should be clamped to global bbox
        assert_eq!(unit.fetch_min_x, 0); // Can't go below 0
        assert_eq!(unit.fetch_max_x, 575); // 511 + 64
        assert_eq!(unit.fetch_min_z, 0);
        assert_eq!(unit.fetch_max_z, 575);
    }

    #[test]
    fn test_compute_processing_units() {
        // A 2x2 region area
        let global_bbox = make_test_bbox(0, 1023, 0, 1023);
        let units = compute_processing_units(&global_bbox, 64, 1);

        assert_eq!(units.len(), 4); // 2x2 = 4 regions
    }
    
    #[test]
    fn test_compute_processing_units_batched() {
        // A 2x2 region area with batch size 2 should create 1 unit
        let global_bbox = make_test_bbox(0, 1023, 0, 1023);
        let units = compute_processing_units(&global_bbox, 64, 2);

        assert_eq!(units.len(), 1); // All 4 regions in one batch
    }

    #[test]
    fn test_calculate_parallel_threads() {
        // Test default (0 = available - 1)
        let threads = calculate_parallel_threads(0);
        assert!(threads >= 1);

        // Test explicit request
        let threads = calculate_parallel_threads(2);
        assert!(threads >= 1 && threads <= 2);
    }
}
