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
    pub fn count_in_range(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> (usize, usize) {
        let mut urban_count = 0usize;
        let mut total_count = 0usize;

        for z in min_z..=max_z {
            // Calculate local z coordinate
            let local_z = i64::from(z) - i64::from(self.min_z);
            if local_z < 0 || local_z >= self.height as i64 {
                // Row is out of bounds, still counts toward total
                total_count += (max_x - min_x + 1) as usize;
                continue;
            }
            let local_z = local_z as usize;

            // Calculate x range in local coordinates
            let local_min_x = (i64::from(min_x) - i64::from(self.min_x)).max(0) as usize;
            let local_max_x = ((i64::from(max_x) - i64::from(self.min_x)) as usize).min(self.width - 1);

            // Count out-of-bounds x coordinates toward total
            let x_start_offset = (i64::from(self.min_x) - i64::from(min_x)).max(0) as usize;
            let x_end_offset = (i64::from(max_x) - i64::from(self.min_x) - (self.width as i64 - 1)).max(0) as usize;
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
                let mask = ((1u16 << (end_bit_in_byte - start_bit_in_byte + 1)) - 1) as u8;
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
                let last_mask = (1u8 << (end_bit_in_byte + 1)) - 1; // bits 0 to end_bit
                urban_count += (last_byte & last_mask).count_ones() as usize;
            }
        }

        (urban_count, total_count)
    }
}

/// Type alias for building footprint bitmap (for backwards compatibility).
pub type BuildingFootprintBitmap = CoordinateBitmap;

/// Bitmap tracking urban coverage (buildings, roads, paved areas, etc.)
/// Used to determine if a boundary area is actually urbanized.
pub type UrbanCoverageBitmap = CoordinateBitmap;

/// Grid-based urban density map for efficient per-coordinate urban checks.
///
/// Divides the world into cells and pre-calculates the urban density of each cell.
/// Uses distance-based smoothing to create organic boundaries around urban areas
/// instead of blocky grid edges.
pub struct UrbanDensityGrid {
    /// Density value (0.0 to 1.0) for each cell, stored in row-major order
    cells: Vec<f32>,
    /// Size of each cell in blocks
    cell_size: i32,
    /// Minimum x coordinate of the grid (world coordinates)
    min_x: i32,
    /// Minimum z coordinate of the grid (world coordinates)
    min_z: i32,
    /// Number of cells in the x direction
    width: usize,
    /// Number of cells in the z direction
    height: usize,
    /// Density threshold for considering a cell "urban"
    threshold: f32,
    /// Buffer distance in blocks around urban areas
    buffer_radius: i32,
}

impl UrbanDensityGrid {
    /// Cell size in blocks
    const DEFAULT_CELL_SIZE: i32 = 64;
    /// Default density threshold (25%)
    const DEFAULT_THRESHOLD: f32 = 0.25;
    /// Buffer radius around urban areas in blocks
    const DEFAULT_BUFFER_RADIUS: i32 = 20;

    /// Creates a new urban density grid from the urban coverage bitmap.
    pub fn from_coverage(coverage: &UrbanCoverageBitmap, xzbbox: &XZBBox) -> Self {
        let cell_size = Self::DEFAULT_CELL_SIZE;
        let min_x = xzbbox.min_x();
        let min_z = xzbbox.min_z();

        // Calculate grid dimensions (round up to cover entire bbox)
        // Use i64 to avoid overflow when world spans more than i32::MAX in either dimension
        let world_width = i64::from(xzbbox.max_x()) - i64::from(min_x) + 1;
        let world_height = i64::from(xzbbox.max_z()) - i64::from(min_z) + 1;
        let width = ((world_width + i64::from(cell_size) - 1) / i64::from(cell_size)) as usize;
        let height = ((world_height + i64::from(cell_size) - 1) / i64::from(cell_size)) as usize;

        // Calculate density for each cell using efficient bitmap counting
        let cell_count = width
            .checked_mul(height)
            .expect("UrbanDensityGrid: grid dimensions too large");
        let mut cells = vec![0.0f32; cell_count];

        for cell_z in 0..height {
            for cell_x in 0..width {
                // Use i64 for intermediate calculations to prevent overflow
                let cell_min_x = (i64::from(min_x) + (cell_x as i64) * i64::from(cell_size)) as i32;
                let cell_min_z = (i64::from(min_z) + (cell_z as i64) * i64::from(cell_size)) as i32;
                let cell_max_x = (cell_min_x + cell_size - 1).min(xzbbox.max_x());
                let cell_max_z = (cell_min_z + cell_size - 1).min(xzbbox.max_z());

                // Use optimized bitmap counting instead of iterating every coordinate
                let (urban_count, total_count) =
                    coverage.count_in_range(cell_min_x, cell_min_z, cell_max_x, cell_max_z);

                let density = if total_count > 0 {
                    urban_count as f32 / total_count as f32
                } else {
                    0.0
                };

                cells[cell_z * width + cell_x] = density;
            }
        }

        Self {
            cells,
            cell_size,
            min_x,
            min_z,
            width,
            height,
            threshold: Self::DEFAULT_THRESHOLD,
            buffer_radius: Self::DEFAULT_BUFFER_RADIUS,
        }
    }

    /// Converts world coordinates to cell coordinates.
    #[inline]
    fn coord_to_cell(&self, x: i32, z: i32) -> (i32, i32) {
        let cell_x = (x - self.min_x) / self.cell_size;
        let cell_z = (z - self.min_z) / self.cell_size;
        (cell_x, cell_z)
    }

    /// Checks if a cell is considered urban (above density threshold).
    #[inline]
    fn is_urban_cell(&self, cell_x: i32, cell_z: i32) -> bool {
        if cell_x < 0 || cell_z < 0 {
            return false;
        }
        let cx = cell_x as usize;
        let cz = cell_z as usize;
        if cx >= self.width || cz >= self.height {
            return false;
        }
        self.cells[cz * self.width + cx] >= self.threshold
    }

    /// Determines if a coordinate should have stone ground placed.
    ///
    /// Uses distance-based smoothing: a point gets stone if it's within
    /// `buffer_radius` blocks of any urban cell's edge.
    #[inline]
    pub fn should_place_stone(&self, x: i32, z: i32) -> bool {
        let (cell_x, cell_z) = self.coord_to_cell(x, z);

        // If this cell is urban, always place stone
        if self.is_urban_cell(cell_x, cell_z) {
            return true;
        }

        // Check distance to nearby urban cells
        // We only need to check cells within buffer_radius distance
        let cells_to_check = (self.buffer_radius / self.cell_size) + 2;
        let buffer_sq = self.buffer_radius * self.buffer_radius;

        for dz in -cells_to_check..=cells_to_check {
            for dx in -cells_to_check..=cells_to_check {
                let check_x = cell_x + dx;
                let check_z = cell_z + dz;

                if self.is_urban_cell(check_x, check_z) {
                    // Calculate distance from point to nearest edge of this urban cell
                    // Use i64 for intermediate calculations to prevent overflow
                    let cell_min_x = (i64::from(self.min_x)
                        + i64::from(check_x) * i64::from(self.cell_size))
                        as i32;
                    let cell_max_x = cell_min_x + self.cell_size - 1;
                    let cell_min_z = (i64::from(self.min_z)
                        + i64::from(check_z) * i64::from(self.cell_size))
                        as i32;
                    let cell_max_z = cell_min_z + self.cell_size - 1;

                    // Distance to nearest point on the cell's bounding box
                    let nearest_x = x.clamp(cell_min_x, cell_max_x);
                    let nearest_z = z.clamp(cell_min_z, cell_max_z);

                    let dist_x = x - nearest_x;
                    let dist_z = z - nearest_z;
                    let dist_sq = dist_x * dist_x + dist_z * dist_z;

                    if dist_sq <= buffer_sq {
                        return true;
                    }
                }
            }
        }

        false
    }
}

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

    /// Collects all urban coverage coordinates from the pre-computed cache.
    ///
    /// Urban coverage includes buildings, roads (as line areas), and urban landuse types.
    /// This is used to determine if a boundary area is truly urbanized or just rural
    /// land that happens to be within administrative city limits.
    ///
    /// # Coverage includes:
    /// - Buildings and building:parts
    /// - Urban landuse types: residential, commercial, industrial, retail, etc.
    /// - Amenities with areas (parking lots, schools, etc.)
    ///
    /// # Note on highways:
    /// Linear highways are NOT included because they use bresenham lines, not flood fill.
    /// However, urban areas typically have enough buildings + urban landuse to provide
    /// adequate coverage signal.
    pub fn collect_urban_coverage(
        &self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
    ) -> UrbanCoverageBitmap {
        let mut coverage = UrbanCoverageBitmap::new(xzbbox);

        for element in elements {
            match element {
                ProcessedElement::Way(way) => {
                    // Check if this is an urban element
                    if Self::is_urban_coverage_element(way) {
                        if let Some(cached) = self.way_cache.get(&way.id) {
                            for &(x, z) in cached {
                                coverage.set(x, z);
                            }
                        }
                    }
                }
                ProcessedElement::Relation(rel) => {
                    // Check buildings
                    if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                        for member in &rel.members {
                            if member.role == ProcessedMemberRole::Outer {
                                if let Some(cached) = self.way_cache.get(&member.way.id) {
                                    for &(x, z) in cached {
                                        coverage.set(x, z);
                                    }
                                }
                            }
                        }
                    }
                    // Check urban landuse relations
                    else if let Some(landuse) = rel.tags.get("landuse") {
                        if Self::is_urban_landuse(landuse) {
                            for member in &rel.members {
                                if member.role == ProcessedMemberRole::Outer {
                                    if let Some(cached) = self.way_cache.get(&member.way.id) {
                                        for &(x, z) in cached {
                                            coverage.set(x, z);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        coverage
    }

    /// Checks if a way element contributes to urban coverage.
    fn is_urban_coverage_element(way: &ProcessedWay) -> bool {
        // Buildings are always urban
        if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
            return true;
        }

        // Urban landuse types
        if let Some(landuse) = way.tags.get("landuse") {
            if Self::is_urban_landuse(landuse) {
                return true;
            }
        }

        // Amenities with areas (parking, schools, etc.)
        if way.tags.contains_key("amenity") {
            return true;
        }

        // Highway areas (pedestrian plazas, etc.)
        if way.tags.contains_key("highway")
            && way.tags.get("area").map(|v| v == "yes").unwrap_or(false)
        {
            return true;
        }

        false
    }

    /// Checks if a landuse type is considered urban.
    fn is_urban_landuse(landuse: &str) -> bool {
        matches!(
            landuse,
            "residential"
                | "commercial"
                | "industrial"
                | "retail"
                | "railway"
                | "construction"
                | "education"
                | "religious"
                | "military"
        )
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
