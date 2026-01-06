//! Pre-computed flood fill cache for parallel polygon filling.
//!
//! This module provides a way to pre-compute all flood fill operations in parallel
//! before the main element processing loop, then retrieve cached results during
//! sequential processing.

use crate::floodfill::flood_fill_area;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use fnv::FnvHashMap;
use rayon::prelude::*;
use std::time::Duration;

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

    /// Returns the number of cached way entries.
    pub fn way_count(&self) -> usize {
        self.way_cache.len()
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
            println!(
                "Configured thread pool: {} threads ({}% of {} cores)",
                target_threads,
                (cpu_fraction * 100.0) as u32,
                available_cores
            );
        }
        Err(_) => {
            // Thread pool already configured
        }
    }
}
