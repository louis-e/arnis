//! Pre-computed flood fill cache for parallel polygon filling.
//!
//! This module provides a way to pre-compute all flood fill operations in parallel
//! before the main element processing loop, then retrieve cached results during
//! sequential processing.

use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedWay};
use fnv::FnvHashMap;
use rayon::prelude::*;
use std::time::Duration;

/// A memory-efficient bitmap for storing coordinates.
///
/// Instead of storing each coordinate individually (~24 bytes per entry in a HashSet),
/// this uses 1 bit per coordinate in the world bounds, reducing memory usage by ~200x.
///
/// For a world of size W x H blocks, the bitmap uses only (W * H) / 8 bytes.
pub struct CoordinateBitmap {
    /// The bitmap data, where each bit represents one (x, z) coordinate
    bits: Vec<u8>,
    /// Minimum x coordinate (offset for indexing)
    min_x: i32,
    /// Minimum z coordinate (offset for indexing)
    min_z: i32,
    /// Width of the world (max_x - min_x + 1)
    width: usize,
    /// Height of the world (max_z - min_z + 1)
    #[allow(dead_code)]
    height: usize,
    /// Number of coordinates marked
    count: usize,
}

impl CoordinateBitmap {
    /// Creates a new empty bitmap covering the given world bounds.
    pub fn new(xzbbox: &XZBBox) -> Self {
        let min_x = xzbbox.min_x();
        let min_z = xzbbox.min_z();
        // Use i64 to avoid overflow when world spans more than i32::MAX in either dimension
        let width = (i64::from(xzbbox.max_x()) - i64::from(min_x) + 1) as usize;
        let height = (i64::from(xzbbox.max_z()) - i64::from(min_z) + 1) as usize;

        // Calculate number of bytes needed (round up to nearest byte)
        let total_bits = width
            .checked_mul(height)
            .expect("CoordinateBitmap: world size too large (width * height overflowed)");
        let num_bytes = total_bits.div_ceil(8);

        Self {
            bits: vec![0u8; num_bytes],
            min_x,
            min_z,
            width,
            height,
            count: 0,
        }
    }

    /// Converts (x, z) coordinate to bit index, returning None if out of bounds.
    #[inline]
    fn coord_to_index(&self, x: i32, z: i32) -> Option<usize> {
        // Use i64 arithmetic to avoid overflow when coordinates span large ranges
        let local_x = i64::from(x) - i64::from(self.min_x);
        let local_z = i64::from(z) - i64::from(self.min_z);

        if local_x < 0 || local_z < 0 {
            return None;
        }

        let local_x = local_x as usize;
        let local_z = local_z as usize;

        if local_x >= self.width || local_z >= self.height {
            return None;
        }

        // Safe: bounds checks above ensure this won't overflow (max = total_bits - 1)
        Some(local_z * self.width + local_x)
    }

    /// Sets a coordinate.
    #[inline]
    pub fn set(&mut self, x: i32, z: i32) {
        if let Some(bit_index) = self.coord_to_index(x, z) {
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;

            // Safety: coord_to_index already validates bounds, so byte_index is always valid
            let mask = 1u8 << bit_offset;
            // Only increment count if bit wasn't already set
            if self.bits[byte_index] & mask == 0 {
                self.bits[byte_index] |= mask;
                self.count += 1;
            }
        }
    }

    /// Checks if a coordinate is set.
    #[inline]
    pub fn contains(&self, x: i32, z: i32) -> bool {
        if let Some(bit_index) = self.coord_to_index(x, z) {
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;

            // Safety: coord_to_index already validates bounds, so byte_index is always valid
            return (self.bits[byte_index] >> bit_offset) & 1 == 1;
        }
        false
    }

    /// Returns true if no coordinates are marked.
    #[must_use]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns the number of coordinates that are set.
    #[inline]
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.count
    }

    /// Counts how many coordinates from the given iterator are set in this bitmap.
    #[inline]
    #[allow(dead_code)]
    pub fn count_contained<'a, I>(&self, coords: I) -> usize
    where
        I: Iterator<Item = &'a (i32, i32)>,
    {
        coords.filter(|(x, z)| self.contains(*x, *z)).count()
    }

    /// Counts the number of set bits in a rectangular range.
    ///
    /// This is optimized to iterate row-by-row and use `count_ones()` on bytes
    /// where possible, which is much faster than checking individual coordinates.
    ///
    /// Returns `(urban_count, total_count)` for the given range.
    #[inline]
    #[allow(dead_code)]
    pub fn count_in_range(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> (usize, usize) {
        let mut urban_count = 0usize;
        let mut total_count = 0usize;

        for z in min_z..=max_z {
            // Calculate local z coordinate
            let local_z = i64::from(z) - i64::from(self.min_z);
            if local_z < 0 || local_z >= self.height as i64 {
                // Row is out of bounds, still counts toward total
                total_count += (i64::from(max_x) - i64::from(min_x) + 1) as usize;
                continue;
            }
            let local_z = local_z as usize;

            // Calculate x range in local coordinates
            let local_min_x = (i64::from(min_x) - i64::from(self.min_x)).max(0) as usize;
            let local_max_x =
                ((i64::from(max_x) - i64::from(self.min_x)) as usize).min(self.width - 1);

            // Count out-of-bounds x coordinates toward total
            let x_start_offset = (i64::from(self.min_x) - i64::from(min_x)).max(0) as usize;
            let x_end_offset = (i64::from(max_x) - i64::from(self.min_x) - (self.width as i64 - 1))
                .max(0) as usize;
            total_count += x_start_offset + x_end_offset;

            if local_min_x > local_max_x {
                continue;
            }

            // Process this row
            let row_start_bit = local_z * self.width + local_min_x;
            let row_end_bit = local_z * self.width + local_max_x;
            let num_bits = row_end_bit - row_start_bit + 1;
            total_count += num_bits;

            // Count set bits using byte-wise popcount where possible
            let start_byte = row_start_bit / 8;
            let end_byte = row_end_bit / 8;
            let start_bit_in_byte = row_start_bit % 8;
            let end_bit_in_byte = row_end_bit % 8;

            if start_byte == end_byte {
                // All bits are in the same byte
                let byte = self.bits[start_byte];
                // Create mask for bits from start_bit to end_bit (inclusive)
                let num_bits_in_mask = end_bit_in_byte - start_bit_in_byte + 1;
                let mask = if num_bits_in_mask >= 8 {
                    0xFFu8
                } else {
                    ((1u16 << num_bits_in_mask) - 1) as u8
                };
                let masked = (byte >> start_bit_in_byte) & mask;
                urban_count += masked.count_ones() as usize;
            } else {
                // First partial byte
                let first_byte = self.bits[start_byte];
                let first_mask = !((1u8 << start_bit_in_byte) - 1); // bits from start_bit to 7
                urban_count += (first_byte & first_mask).count_ones() as usize;

                // Full bytes in between
                for byte_idx in (start_byte + 1)..end_byte {
                    urban_count += self.bits[byte_idx].count_ones() as usize;
                }

                // Last partial byte
                let last_byte = self.bits[end_byte];
                // Handle case where end_bit_in_byte is 7 (would overflow 1u8 << 8)
                let last_mask = if end_bit_in_byte >= 7 {
                    0xFFu8
                } else {
                    (1u8 << (end_bit_in_byte + 1)) - 1
                };
                urban_count += (last_byte & last_mask).count_ones() as usize;
            }
        }

        (urban_count, total_count)
    }
}

/// Type alias for building footprint bitmap (for backwards compatibility).
pub type BuildingFootprintBitmap = CoordinateBitmap;

/// A cache of pre-computed flood fill results, keyed by element ID.
pub struct FloodFillCache {
    /// Cached results: element_id -> filled coordinates
    way_cache: FnvHashMap<u64, Vec<(i32, i32)>>,
}

impl FloodFillCache {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self {
            way_cache: FnvHashMap::default(),
        }
    }

    /// Pre-computes flood fills for all elements that need them.
    ///
    /// This runs in parallel using Rayon, taking advantage of multiple CPU cores.
    pub fn precompute(elements: &[ProcessedElement], timeout: Option<&Duration>) -> Self {
        // Collect all ways that need flood fill
        let ways_needing_fill: Vec<&ProcessedWay> = elements
            .iter()
            .filter_map(|el| match el {
                ProcessedElement::Way(way) => {
                    if Self::way_needs_flood_fill(way) {
                        Some(way)
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        // Compute all way flood fills in parallel
        let way_results: Vec<(u64, Vec<(i32, i32)>)> = ways_needing_fill
            .par_iter()
            .map(|way| {
                let polygon_coords: Vec<(i32, i32)> =
                    way.nodes.iter().map(|n| (n.x, n.z)).collect();
                let filled = flood_fill_area(&polygon_coords, timeout);
                (way.id, filled)
            })
            .collect();

        // Build the cache
        let mut cache = Self::new();
        for (id, filled) in way_results {
            cache.way_cache.insert(id, filled);
        }

        cache
    }

    /// Gets cached flood fill result for a way, or computes it if not cached.
    ///
    /// Note: Combined ways created from relations (e.g., in `generate_natural_from_relation`)
    /// will miss the cache and fall back to on-demand computation. This is by design,
    /// these synthetic ways don't exist in the original element list and have relation IDs
    /// rather than way IDs. The individual member ways are still cached.
    pub fn get_or_compute(
        &self,
        way: &ProcessedWay,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        if let Some(cached) = self.way_cache.get(&way.id) {
            // Clone is intentional: each result is typically accessed once during
            // sequential processing, so the cost is acceptable vs Arc complexity
            cached.clone()
        } else {
            // Fallback: compute on demand for synthetic/combined ways from relations
            let polygon_coords: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
            flood_fill_area(&polygon_coords, timeout)
        }
    }

    /// Gets cached flood fill result for a ProcessedElement (Way only).
    /// For Nodes/Relations, returns empty vec.
    pub fn get_or_compute_element(
        &self,
        element: &ProcessedElement,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        match element {
            ProcessedElement::Way(way) => self.get_or_compute(way, timeout),
            _ => Vec::new(),
        }
    }

    /// Determines if a way element needs flood fill based on its tags.
    ///
    /// This checks for tag presence (not specific values) because:
    /// - Only some values within each tag type actually use flood fill
    /// - But caching extra results is harmless (small memory overhead)
    /// - And avoids duplicating value-checking logic from processors
    ///
    /// Covered cases:
    /// - building/building:part -> buildings::generate_buildings (includes bridge)
    /// - landuse -> landuse::generate_landuse
    /// - leisure -> leisure::generate_leisure
    /// - amenity -> amenities::generate_amenities
    /// - natural (except tree) -> natural::generate_natural
    /// - highway with area=yes -> highways::generate_highways (area fill)
    fn way_needs_flood_fill(way: &ProcessedWay) -> bool {
        way.tags.contains_key("building")
            || way.tags.contains_key("building:part")
            || way.tags.contains_key("boundary")
            || way.tags.contains_key("landuse")
            || way.tags.contains_key("leisure")
            || way.tags.contains_key("amenity")
            || way
                .tags
                .get("natural")
                .map(|v| v != "tree")
                .unwrap_or(false)
            // Highway areas (like pedestrian plazas) use flood fill when area=yes
            || (way.tags.contains_key("highway")
                && way.tags.get("area").map(|v| v == "yes").unwrap_or(false))
            // Historic tomb polygons (e.g. tomb=pyramid)
            || way.tags.contains_key("tomb")
    }

    /// Collects all building footprint coordinates from the pre-computed cache.
    ///
    /// This should be called after precompute() and before elements are processed.
    /// Returns a memory-efficient bitmap of all (x, z) coordinates that are part of buildings.
    ///
    /// The bitmap uses only 1 bit per coordinate in the world bounds, compared to ~24 bytes
    /// per entry in a HashSet, reducing memory usage by ~200x for large worlds.
    pub fn collect_building_footprints(
        &self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
    ) -> BuildingFootprintBitmap {
        let mut footprints = BuildingFootprintBitmap::new(xzbbox);

        for element in elements {
            match element {
                ProcessedElement::Way(way) => {
                    if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                        if let Some(cached) = self.way_cache.get(&way.id) {
                            for &(x, z) in cached {
                                footprints.set(x, z);
                            }
                        }
                    }
                }
                ProcessedElement::Relation(rel) => {
                    if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                        for member in &rel.members {
                            // Only treat outer members as building footprints.
                            // Inner members represent courtyards/holes where trees can spawn.
                            if member.role == ProcessedMemberRole::Outer {
                                if let Some(cached) = self.way_cache.get(&member.way.id) {
                                    for &(x, z) in cached {
                                        footprints.set(x, z);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        footprints
    }

    /// Collects centroids of all buildings from the pre-computed cache.
    ///
    /// This is used for urban ground detection - building clusters are identified
    /// using their centroids, and a concave hull is computed around dense clusters
    /// to determine where city ground (smooth stone) should be placed.
    ///
    /// Returns a vector of (x, z) centroid coordinates for all buildings.
    pub fn collect_building_centroids(&self, elements: &[ProcessedElement]) -> Vec<(i32, i32)> {
        let mut centroids = Vec::new();

        for element in elements {
            match element {
                ProcessedElement::Way(way) => {
                    if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                        if let Some(cached) = self.way_cache.get(&way.id) {
                            if let Some(centroid) = Self::compute_centroid(cached) {
                                centroids.push(centroid);
                            }
                        }
                    }
                }
                ProcessedElement::Relation(rel) => {
                    if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                        // For building relations, compute centroid from outer ways
                        let mut all_coords = Vec::new();
                        for member in &rel.members {
                            if member.role == ProcessedMemberRole::Outer {
                                if let Some(cached) = self.way_cache.get(&member.way.id) {
                                    all_coords.extend(cached.iter().copied());
                                }
                            }
                        }
                        if let Some(centroid) = Self::compute_centroid(&all_coords) {
                            centroids.push(centroid);
                        }
                    }
                }
                _ => {}
            }
        }

        centroids
    }

    /// Computes the centroid of a set of coordinates.
    fn compute_centroid(coords: &[(i32, i32)]) -> Option<(i32, i32)> {
        if coords.is_empty() {
            return None;
        }
        let sum_x: i64 = coords.iter().map(|(x, _)| i64::from(*x)).sum();
        let sum_z: i64 = coords.iter().map(|(_, z)| i64::from(*z)).sum();
        let len = coords.len() as i64;
        Some(((sum_x / len) as i32, (sum_z / len) as i32))
    }

    /// Removes a way's cached flood fill result, freeing memory.
    ///
    /// Call this after processing an element to release its cached data.
    pub fn remove_way(&mut self, way_id: u64) {
        self.way_cache.remove(&way_id);
    }

    /// Removes all cached flood fill results for ways in a relation.
    ///
    /// Relations contain multiple ways, so we need to remove all of them.
    pub fn remove_relation_ways(&mut self, way_ids: &[u64]) {
        for &id in way_ids {
            self.way_cache.remove(&id);
        }
    }
}

impl Default for FloodFillCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Configures the global Rayon thread pool with a CPU usage cap.
///
/// Call this once at startup before any parallel operations.
///
/// # Arguments
/// * `cpu_fraction` - Fraction of available cores to use (e.g., 0.9 for 90%).
///   Values are clamped to the range [0.1, 1.0].
pub fn configure_rayon_thread_pool(cpu_fraction: f64) {
    // Clamp cpu_fraction to valid range
    let cpu_fraction = cpu_fraction.clamp(0.1, 1.0);

    let available_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let target_threads = ((available_cores as f64) * cpu_fraction).floor() as usize;
    let target_threads = target_threads.max(1); // At least 1 thread

    // Only configure if we haven't already (this can only be called once)
    match rayon::ThreadPoolBuilder::new()
        .num_threads(target_threads)
        .build_global()
    {
        Ok(()) => {
            // Successfully configured (silent to avoid cluttering output)
        }
        Err(_) => {
            // Thread pool already configured
        }
    }
}
