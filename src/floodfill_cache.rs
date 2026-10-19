//! Flood fill cache shared by the element handlers and the tile threads.
//!
//! Fills are computed on first read and released after their last reader, so only the
//! fills of the tiles in flight are held at once.

use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill::{flood_fill_area, MAX_FLOOD_FILL_AREA};
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedWay};
use fnv::FnvHashMap;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Shared, reference-counted flood fill result.
///
/// Using `Arc<Vec<...>>` lets handlers grab a cached result without deep-copying
/// the coordinate list — a single refcount bump instead of a `Vec::clone` that
/// would memcpy 8 bytes per cell (large forests/farmland easily hit 100k+ cells).
pub type FloodFillResult = Arc<Vec<(i32, i32)>>;

/// Whether a way is a closed ring whose bounding box is past the flood fill's cap.
///
/// Handlers that paint an outline before filling pair this with an empty fill to decide
/// whether to paint at all. Three different things fill to nothing and only one of them is a
/// refusal: an open way like a ridge renders as a line by design, a sliver too thin to hold a
/// lattice point has no interior to find, and a ring past the cap was given up on. Only the
/// last should drop its outline, since a border around ground nothing filled reads as a bug.
pub fn is_oversized_ring(way: &ProcessedWay) -> bool {
    let closed = way.nodes.len() >= 4
        && way.nodes.first().map(|n| (n.x, n.z)) == way.nodes.last().map(|n| (n.x, n.z));
    if !closed {
        return false;
    }

    let (mut min_x, mut max_x) = (i32::MAX, i32::MIN);
    let (mut min_z, mut max_z) = (i32::MAX, i32::MIN);
    for n in &way.nodes {
        min_x = min_x.min(n.x);
        max_x = max_x.max(n.x);
        min_z = min_z.min(n.z);
        max_z = max_z.max(n.z);
    }

    let area = (max_x as i64 - min_x as i64 + 1) * (max_z as i64 - min_z as i64 + 1);
    area > MAX_FLOOD_FILL_AREA
}

/// Returns a reference to a process-wide shared empty flood-fill result.
///
/// Used as the sentinel for Node/Relation elements that don't produce a flood
/// fill, so we don't allocate a fresh empty `Arc<Vec<_>>` on every call.
fn empty_flood_fill_result() -> &'static FloodFillResult {
    static EMPTY: OnceLock<FloodFillResult> = OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(Vec::new()))
}

/// A memory-efficient bitmap for storing coordinates.
///
/// Instead of storing each coordinate individually (~24 bytes per entry in a HashSet),
/// this uses 1 bit per coordinate in the world bounds, reducing memory usage by ~200x.
///
/// For a world of size W x H blocks, the bitmap uses only (W * H) / 8 bytes.
#[derive(Clone)]
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

    /// Creates a zero-size bitmap that contains nothing and allocates no memory.
    pub fn new_empty() -> Self {
        Self {
            bits: Vec::new(),
            min_x: 0,
            min_z: 0,
            width: 0,
            height: 0,
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

    /// Clears a coordinate.
    #[inline]
    pub fn clear(&mut self, x: i32, z: i32) {
        if let Some(bit_index) = self.coord_to_index(x, z) {
            let byte_index = bit_index / 8;
            let mask = 1u8 << (bit_index % 8);
            if self.bits[byte_index] & mask != 0 {
                self.bits[byte_index] &= !mask;
                self.count -= 1;
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

    /// Checks if a coordinate is inside the bitmap and not set yet.
    #[inline]
    fn is_unset_in_bounds(&self, x: i32, z: i32) -> bool {
        self.coord_to_index(x, z)
            .is_some_and(|i| (self.bits[i / 8] >> (i % 8)) & 1 == 0)
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

/// Type alias for the road surface bitmap used by amenity processors.
/// Built by `highways::collect_road_surface_coords` using the same Bresenham +
/// block_range geometry as the renderer, so every placed road/path block coordinate
/// is marked as 1 and everything else is 0.
pub type RoadMaskBitmap = CoordinateBitmap;

/// Type alias for the sealed-surface bitmap: every column owned by a man-made
/// ground cover that vegetation must stay off (roads and paths plus sport
/// pitches, courts, playgrounds and similar paved areas).
///
/// The block under a column is not enough to decide this on its own. OSM
/// surface values map onto blocks that also occur as natural ground, so a
/// `surface=dirt` track and a dirt field look identical to a block check, and
/// an area painted before a smaller one on top of it has already lost its
/// original block by the time the later element runs. The bitmap is resolved
/// once from the element list instead, before anything is placed.
pub type SealedSurfaceBitmap = CoordinateBitmap;

/// Leisure areas rendered as a managed or paved surface. Trees, flowers and
/// grass tufts on these read as a bug, so they join the sealed-surface mask.
const SEALED_LEISURE: &[&str] = &[
    "pitch",
    "track",
    "playground",
    "recreation_ground",
    "schoolyard",
    "ice_rink",
    "water_park",
    "slipway",
    "outdoor_seating",
    "bathing_place",
    "fitness_station",
];

/// Amenity areas that pave their footprint.
const SEALED_AMENITY: &[&str] = &[
    "parking",
    "parking_space",
    "bicycle_parking",
    "motorcycle_parking",
    "fuel",
    "charging_station",
    "car_wash",
    "taxi",
    "marketplace",
];

/// Whether a way paints a sealed surface over its whole footprint.
fn is_sealed_surface_way(way: &ProcessedWay) -> bool {
    if let Some(v) = way.tags.get("leisure") {
        if SEALED_LEISURE.contains(&v.as_str()) {
            return true;
        }
    }
    if let Some(v) = way.tags.get("amenity") {
        if SEALED_AMENITY.contains(&v.as_str()) {
            return true;
        }
    }
    // Pedestrian plazas and other highway=* areas are flood-filled, not stamped
    // along a centerline, so the road mask does not cover them.
    way.tags.contains_key("highway") && way.tags.get("area").is_some_and(|v| v == "yes")
}

/// Paving that yields to planted areas mapped inside it. Pitches, tracks and
/// playgrounds keep their whole footprint.
fn paving_yields_to_nested_areas(way: &ProcessedWay) -> bool {
    (way.tags.contains_key("highway") && way.tags.get("area").is_some_and(|v| v == "yes"))
        || way
            .tags
            .get("amenity")
            .is_some_and(|v| SEALED_AMENITY.contains(&v.as_str()))
        || way.tags.get("leisure").map(String::as_str) == Some("schoolyard")
}

const PLANTED_LANDUSE: &[&str] = &[
    "grass",
    "flowerbed",
    "meadow",
    "greenfield",
    "village_green",
    "forest",
    "orchard",
    "allotments",
    "plant_nursery",
];

/// A lawn, bed, wood or pond, as dispatched by `process_element`.
fn is_planted_way(way: &ProcessedWay) -> bool {
    let tags = &way.tags;
    if ["building", "building:part", "highway"]
        .iter()
        .any(|k| tags.contains_key(*k))
    {
        return false;
    }
    if let Some(landuse) = tags.get("landuse") {
        return PLANTED_LANDUSE.contains(&landuse.as_str());
    }
    if let Some(natural) = tags.get("natural") {
        return !matches!(natural.as_str(), "tree" | "tree_row")
            && tags.get("amenity").map(String::as_str) != Some("fountain");
    }
    !tags.contains_key("amenity")
        && matches!(
            tags.get("leisure").map(String::as_str),
            Some("park" | "garden")
        )
}

/// A cached way fill: computed on first read, dropped once its last reader is done.
enum CachedFill {
    Pending,
    Ready(FloodFillResult),
    Released,
}

/// A way's ring and the fill made from it.
struct WayFill {
    ring: Box<[(i32, i32)]>,
    fill: Mutex<CachedFill>,
}

/// Fills prepared per pass, so a pre-pass never holds every fill of the world at once.
const PREPASS_BATCH: usize = 512;
/// Cells a pass that applies fills in order holds at once (8 bytes each).
const PREPASS_BATCH_CELLS: usize = 1 << 22;

/// Flood fills shared between handlers and tiles, keyed by way ID.
///
/// Each fill is computed on first read and kept until released, so a large world holds
/// only the fills of the tiles in flight rather than every fill at once.
pub struct FloodFillCache {
    /// Ring and fill per way (shared via Arc so handler fetches are O(1) refcount bumps
    /// instead of deep clones). Releasable through `&self`, so tile threads can free
    /// fills mid-run.
    way_cache: FnvHashMap<u64, WayFill>,
    timeout: Option<Duration>,
}

/// Fills a ring, sharing one allocation for every empty result.
fn fill_ring(ring: &[(i32, i32)], timeout: Option<&Duration>) -> FloodFillResult {
    let filled = flood_fill_area(ring, timeout);
    if filled.is_empty() {
        Arc::clone(empty_flood_fill_result())
    } else {
        Arc::new(filled)
    }
}

impl FloodFillCache {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self {
            way_cache: FnvHashMap::default(),
            timeout: None,
        }
    }

    /// Registers every way whose fill a handler may read. Nothing is filled yet.
    pub fn precompute(elements: &[ProcessedElement], timeout: Option<&Duration>) -> Self {
        // Collect all ways that need flood fill
        let mut ways_needing_fill: Vec<&ProcessedWay> = elements
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

        // The outer rings of the relations filled member by member. Every tile a relation
        // overlaps used to fill them again, which for a large beach or dune field meant a
        // full-size fill per tile thread; once here they are shared like a way's.
        let mut seen: fnv::FnvHashSet<u64> = ways_needing_fill.iter().map(|w| w.id).collect();
        for element in elements {
            let ProcessedElement::Relation(rel) = element else {
                continue;
            };
            if !Self::relation_fills_members(rel) {
                continue;
            }
            // Piers and breakwaters also cut their holes out of the fill.
            let holes = Self::is_waterfront(&rel.tags);
            for member in &rel.members {
                let wanted = member.role == ProcessedMemberRole::Outer
                    || (holes && member.role == ProcessedMemberRole::Inner);
                if wanted && seen.insert(member.way.id) {
                    ways_needing_fill.push(&member.way);
                }
            }
        }

        let way_cache = ways_needing_fill
            .into_iter()
            .map(|way| {
                let ring = way.nodes.iter().map(|n| (n.x, n.z)).collect();
                let fill = Mutex::new(CachedFill::Pending);
                (way.id, WayFill { ring, fill })
            })
            .collect();
        Self {
            way_cache,
            timeout: timeout.copied(),
        }
    }

    /// Fill for a registered way id, computed on first read; None when absent or released.
    /// Concurrent readers of a fill still being computed wait for it instead of repeating it.
    pub fn get_cached(&self, way_id: u64) -> Option<FloodFillResult> {
        let way = self.way_cache.get(&way_id)?;
        let mut slot = way.fill.lock().unwrap_or_else(|p| p.into_inner());
        match &*slot {
            CachedFill::Ready(fill) => Some(Arc::clone(fill)),
            CachedFill::Released => None,
            CachedFill::Pending => {
                let fill = fill_ring(&way.ring, self.timeout.as_ref());
                *slot = CachedFill::Ready(Arc::clone(&fill));
                Some(fill)
            }
        }
    }

    /// A registered way's fill for a pre-pass: shared if a reader already made it,
    /// otherwise computed without being kept.
    fn transient_fill(&self, way_id: u64) -> Option<FloodFillResult> {
        let way = self.way_cache.get(&way_id)?;
        if let CachedFill::Ready(fill) = &*way.fill.lock().unwrap_or_else(|p| p.into_inner()) {
            return Some(Arc::clone(fill));
        }
        Some(fill_ring(&way.ring, self.timeout.as_ref()))
    }

    /// Whether a fill is registered for this way id, released or not.
    pub fn contains(&self, way_id: u64) -> bool {
        self.way_cache.contains_key(&way_id)
    }

    /// Frees a way's fill once nothing reads it again; `get_cached` is None afterwards.
    pub fn release(&self, way_id: u64) {
        if let Some(way) = self.way_cache.get(&way_id) {
            *way.fill.lock().unwrap_or_else(|p| p.into_inner()) = CachedFill::Released;
        }
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
    ) -> FloodFillResult {
        if let Some(cached) = self.get_cached(way.id) {
            // Cheap refcount bump — the underlying Vec is shared between the
            // cache and the caller.
            cached
        } else {
            // Fallback: compute on demand for synthetic/combined ways from relations.
            // These are rare (only relations with tag-inherited members), so the
            // extra Arc allocation here is fine. Empty results still go through
            // the shared sentinel to stay consistent with the cached path.
            let polygon_coords: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
            let filled = flood_fill_area(&polygon_coords, timeout);
            if filled.is_empty() {
                Arc::clone(empty_flood_fill_result())
            } else {
                Arc::new(filled)
            }
        }
    }

    /// Gets cached flood fill result for a ProcessedElement (Way only).
    /// For Nodes/Relations, returns a process-wide shared empty vec (no alloc).
    pub fn get_or_compute_element(
        &self,
        element: &ProcessedElement,
        timeout: Option<&Duration>,
    ) -> FloodFillResult {
        match element {
            ProcessedElement::Way(way) => self.get_or_compute(way, timeout),
            _ => Arc::clone(empty_flood_fill_result()),
        }
    }

    /// Determines if a way element needs flood fill based on its tags.
    ///
    /// Mostly tag presence, so most values are cached whether read or not; the big
    /// polygons whose fill is never read are left out by `fill_goes_unread`.
    ///
    /// Covered cases:
    /// - building/building:part -> buildings::generate_buildings (includes bridge)
    /// - landuse -> landuse::generate_landuse
    /// - leisure -> leisure::generate_leisure
    /// - amenity -> amenities::generate_amenities
    /// - natural (except tree and tree_row) -> natural::generate_natural
    /// - highway with area=yes -> highways::generate_highways (area fill)
    fn way_needs_flood_fill(way: &ProcessedWay) -> bool {
        (way.tags.contains_key("building")
            || way.tags.contains_key("building:part")
            || way.tags.contains_key("landuse")
            || way.tags.contains_key("leisure")
            || way.tags.contains_key("amenity")
            || way
                .tags
                .get("natural")
                .map(|v| v != "tree" && v != "tree_row")
                .unwrap_or(false)
            // Highway areas (like pedestrian plazas) use flood fill when area=yes
            || (way.tags.contains_key("highway")
                && way.tags.get("area").map(|v| v == "yes").unwrap_or(false))
            // Historic tomb polygons (e.g. tomb=pyramid)
            || way.tags.get("tomb").map(|v| v == "pyramid").unwrap_or(false)
            // Pier and breakwater areas -> waterfront
            || Self::is_waterfront(&way.tags)
            // Solar farm areas -> power::generate_solar_farm
            || (way.tags.get("power").map(String::as_str) == Some("generator")
                && way.tags.get("generator:source").map(String::as_str) == Some("solar")))
            && !Self::fill_goes_unread(way)
    }

    /// Ways `process_element` hands to a branch that never reads their fill, and no pre-pass
    /// reads either. Residential and commercial landuse are the largest polygons around.
    fn fill_goes_unread(way: &ProcessedWay) -> bool {
        let tags = &way.tags;
        if ["building", "building:part", "highway", "power"]
            .iter()
            .any(|k| tags.contains_key(*k))
            || Self::is_waterfront(tags)
            || is_sealed_surface_way(way)
        {
            return false;
        }
        if let Some(landuse) = tags.get("landuse") {
            return matches!(landuse.as_str(), "residential" | "commercial");
        }
        if tags.contains_key("natural")
            && tags.get("amenity").map(String::as_str) != Some("fountain")
        {
            return false;
        }
        tags.get("amenity").is_some_and(|amenity| {
            !matches!(
                amenity.as_str(),
                "bicycle_parking" | "shelter" | "fountain" | "parking"
            )
        })
    }

    /// Relations `process_element` renders by filling each outer member: natural, landuse
    /// that paints ground, parks, piers and breakwaters. Mirrors its dispatch; keep the two
    /// in sync.
    fn relation_fills_members(rel: &crate::osm_parser::ProcessedRelation) -> bool {
        let tags = &rel.tags;
        if tags.contains_key("building")
            || tags.contains_key("building:part")
            || tags.get("type").map(String::as_str) == Some("building")
            || tags.contains_key("water")
            || matches!(
                tags.get("natural").map(String::as_str),
                Some("water" | "bay")
            )
        {
            return false;
        }
        if tags.contains_key("natural") {
            return true;
        }
        if let Some(landuse) = tags.get("landuse") {
            // generate_landuse returns before filling these.
            return !matches!(landuse.as_str(), "residential" | "commercial");
        }
        tags.get("leisure").map(String::as_str) == Some("park") || Self::is_waterfront(tags)
    }

    /// Piers, breakwaters and groynes, which the waterfront code fills as decks and mounds.
    fn is_waterfront(tags: &std::collections::HashMap<String, String>) -> bool {
        matches!(
            tags.get("man_made").map(String::as_str),
            Some("pier" | "breakwater" | "groyne")
        )
    }

    /// Collects all building footprint coordinates, filling in batches that are dropped
    /// after use. Call before elements are processed.
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

        // Relations first (outer fills, courtyards clear), ways second so courtyard buildings re-mark.
        for element in elements {
            if let ProcessedElement::Relation(rel) = element {
                let is_building = (rel.tags.contains_key("building")
                    || rel.tags.contains_key("building:part")
                    || rel.tags.get("type").map(|t| t.as_str()) == Some("building"))
                    && !crate::element_processing::buildings::is_underground_building(&rel.tags);
                if !is_building {
                    continue;
                }
                for_relation_ring_cells(rel, ProcessedMemberRole::Outer, xzbbox, |x, z| {
                    footprints.set(x, z)
                });
                for_relation_ring_cells(rel, ProcessedMemberRole::Inner, xzbbox, |x, z| {
                    footprints.clear(x, z)
                });
            }
        }

        let buildings: Vec<u64> = elements
            .iter()
            .filter_map(|element| match element {
                ProcessedElement::Way(way)
                    if (way.tags.contains_key("building")
                        || way.tags.contains_key("building:part"))
                        && !crate::element_processing::buildings::is_underground_building(
                            &way.tags,
                        ) =>
                {
                    Some(way.id)
                }
                _ => None,
            })
            .collect();
        for batch in buildings.chunks(PREPASS_BATCH) {
            let fills: Vec<Option<FloodFillResult>> = batch
                .par_iter()
                .map(|&id| self.transient_fill(id))
                .collect();
            for fill in fills.iter().flatten() {
                for &(x, z) in fill.iter() {
                    footprints.set(x, z);
                }
            }
        }

        footprints
    }

    /// Builds the sealed-surface mask: the road mask plus every cached footprint
    /// of a leisure, amenity or highway area that paves its ground, less the
    /// planted areas mapped inside paving.
    ///
    /// Returns `None` when no such area contributes a column the roads do not
    /// already own, so the caller can share the road mask instead of paying for a
    /// second full-world bitmap. Only fill sizes are kept between passes: each fill is made
    /// again, a bounded batch at a time, when its layer is applied.
    pub fn collect_sealed_surfaces(
        &self,
        elements: &[ProcessedElement],
        roads: &RoadMaskBitmap,
    ) -> Option<SealedSurfaceBitmap> {
        let mut planted_ids: Vec<u64> = Vec::new();
        let mut sealed_ids: Vec<(u64, bool)> = Vec::new();
        for element in elements {
            let ProcessedElement::Way(w) = element else {
                continue;
            };
            if !self.contains(w.id) {
                continue;
            }
            if is_sealed_surface_way(w) {
                sealed_ids.push((w.id, paving_yields_to_nested_areas(w)));
            } else if is_planted_way(w) {
                planted_ids.push(w.id);
            }
        }

        // A tagged area whose fill came back empty (degenerate ring, fill timeout) or that
        // sits entirely on columns the roads already own would clone a full second bitmap
        // for nothing, so bail out before allocating.
        let off_roads = AtomicBool::new(false);
        let sealed_lens: Vec<Option<usize>> = sealed_ids
            .par_iter()
            .map(|&(id, _)| {
                let fill = self.transient_fill(id)?;
                if !off_roads.load(Ordering::Relaxed)
                    && fill.iter().any(|&(x, z)| roads.is_unset_in_bounds(x, z))
                {
                    off_roads.store(true, Ordering::Relaxed);
                }
                Some(fill.len())
            })
            .collect();
        if !off_roads.into_inner() {
            return None;
        }
        // (cells, paved, way id)
        let mut layers: Vec<(usize, bool, u64)> = Vec::new();
        let mut managed: Vec<(usize, bool, u64)> = Vec::new();
        for (&(id, yields), len) in sealed_ids.iter().zip(sealed_lens) {
            match len {
                Some(len) if yields => layers.push((len, true, id)),
                Some(len) => managed.push((len, true, id)),
                None => {}
            }
        }

        // Anything as large as all the paving changes nothing, so planted areas that big are
        // dropped as soon as their size is known.
        let largest = layers.iter().map(|l| l.0).max().unwrap_or(0);
        if largest > 0 {
            let planted_lens: Vec<Option<usize>> = planted_ids
                .par_iter()
                .map(|&id| self.transient_fill(id).map(|f| f.len()))
                .collect();
            layers.extend(
                planted_ids
                    .iter()
                    .zip(planted_lens)
                    .filter_map(|(&id, len)| {
                        len.filter(|&len| len < largest).map(|len| (len, false, id))
                    }),
            );
        }

        // Largest first, so each column takes the state of the smallest area over it; paving
        // wins a tie. The other sealed areas pave their whole footprint after that.
        layers.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        layers.extend(managed);

        let mut mask = roads.clone();
        let mut rest = &layers[..];
        while !rest.is_empty() {
            let mut cells = 0;
            let n = rest
                .iter()
                .take_while(|l| {
                    cells += l.0;
                    cells <= PREPASS_BATCH_CELLS
                })
                .count()
                .max(1);
            let (batch, tail) = rest.split_at(n);
            let fills: Vec<Option<FloodFillResult>> = batch
                .par_iter()
                .map(|&(_, _, id)| self.transient_fill(id))
                .collect();
            for (&(_, paved, _), fill) in batch.iter().zip(fills) {
                for &(x, z) in fill.iter().flat_map(|f| f.iter()) {
                    if paved {
                        mask.set(x, z);
                    } else if !roads.contains(x, z) {
                        mask.clear(x, z);
                    }
                }
            }
            rest = tail;
        }

        Some(mask)
    }

    /// Removes a way's cached flood fill result, freeing memory.
    ///
    /// Call this after processing an element to release its cached data.
    pub fn remove_way(&mut self, way_id: u64) {
        self.way_cache.remove(&way_id);
    }
}

impl Default for FloodFillCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Applies a callback to the flood-filled cells of a relation's merged, clipped, closed rings.
fn for_relation_ring_cells(
    rel: &crate::osm_parser::ProcessedRelation,
    role: ProcessedMemberRole,
    xzbbox: &XZBBox,
    mut apply: impl FnMut(i32, i32),
) {
    let mut rings: Vec<Vec<crate::osm_parser::ProcessedNode>> = rel
        .members
        .iter()
        .filter(|m| m.role == role)
        .map(|m| m.way.nodes.clone())
        .collect();
    crate::element_processing::merge_way_segments(&mut rings);

    for ring in rings {
        let ring = crate::clipping::clip_way_to_bbox(&ring, xzbbox);
        if ring.len() < 4 {
            continue;
        }
        let mut coords: Vec<(i32, i32)> = ring.iter().map(|n| (n.x, n.z)).collect();
        if coords.first() != coords.last() {
            let (fx, fz) = coords[0];
            let (lx, lz) = *coords.last().unwrap();
            if (fx - lx).abs() <= 1 && (fz - lz).abs() <= 1 {
                coords.push((fx, fz));
            } else {
                continue;
            }
        }
        for (x, z) in flood_fill_area(&coords, None) {
            apply(x, z);
        }
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
    // Respect an explicit RAYON_NUM_THREADS override (benchmarking / power users);
    // leaving the global pool unbuilt lets rayon honor the env var.
    if std::env::var_os("RAYON_NUM_THREADS").is_some() {
        return;
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::ProcessedNode;
    use std::collections::HashMap;

    fn way(points: &[(i32, i32)]) -> ProcessedWay {
        ProcessedWay {
            id: 1,
            nodes: points
                .iter()
                .map(|&(x, z)| ProcessedNode {
                    id: 0,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: HashMap::new(),
        }
    }

    #[test]
    fn a_ring_past_the_cap_is_oversized() {
        assert!(is_oversized_ring(&way(&[
            (0, 0),
            (6000, 0),
            (6000, 6000),
            (0, 6000),
            (0, 0)
        ])));
    }

    #[test]
    fn an_open_way_is_never_oversized() {
        // A ridge or cliff traced as a line fills to nothing by design, however far it runs.
        assert!(!is_oversized_ring(&way(&[
            (0, 0),
            (6000, 2000),
            (3000, 6000),
            (6000, 6000)
        ])));
    }

    #[test]
    fn a_thin_sliver_ring_is_not_oversized() {
        assert!(!is_oversized_ring(&way(&[
            (0, 0),
            (400, 0),
            (400, 1),
            (0, 1),
            (0, 0)
        ])));
    }

    #[test]
    fn a_normal_ring_is_not_oversized() {
        assert!(!is_oversized_ring(&way(&[
            (0, 0),
            (300, 0),
            (300, 300),
            (0, 300),
            (0, 0)
        ])));
    }

    fn tagged_way(id: u64, points: &[(i32, i32)], tags: &[(&str, &str)]) -> ProcessedWay {
        let mut w = way(points);
        w.id = id;
        w.tags = tags
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect();
        w
    }

    #[test]
    fn sealed_surfaces_cover_pitches_and_keep_the_roads() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 99, 99).unwrap();
        let pitch = tagged_way(
            7,
            &[(10, 10), (30, 10), (30, 25), (10, 25), (10, 10)],
            &[("leisure", "pitch"), ("sport", "basketball")],
        );
        let meadow = tagged_way(
            8,
            &[(50, 50), (70, 50), (70, 70), (50, 70), (50, 50)],
            &[("landuse", "meadow")],
        );
        let elements = vec![ProcessedElement::Way(pitch), ProcessedElement::Way(meadow)];

        let cache = FloodFillCache::precompute(&elements, None);
        let mut roads = RoadMaskBitmap::new(&xzbbox);
        roads.set(80, 80);

        let sealed = cache
            .collect_sealed_surfaces(&elements, &roads)
            .expect("a pitch is a sealed surface");

        assert!(sealed.contains(80, 80), "road columns stay marked");
        assert!(sealed.contains(20, 17), "the pitch interior is sealed");
        assert!(!sealed.contains(60, 60), "a meadow is not a sealed surface");
        assert!(!sealed.contains(5, 5), "untouched ground stays open");
    }

    #[test]
    fn a_lawn_inside_paving_keeps_its_ground_but_not_inside_a_pitch() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 99, 99).unwrap();
        let square = |id, (x0, z0): (i32, i32), side: i32, tags: &[(&str, &str)]| {
            let (x1, z1) = (x0 + side, z0 + side);
            ProcessedElement::Way(tagged_way(
                id,
                &[(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)],
                tags,
            ))
        };
        let elements = vec![
            square(1, (0, 0), 40, &[("amenity", "parking")]),
            square(2, (10, 10), 10, &[("landuse", "grass")]),
            // A car park inside the lawn is paved again.
            square(3, (12, 12), 3, &[("amenity", "parking")]),
            square(4, (50, 50), 40, &[("leisure", "pitch")]),
            square(5, (60, 60), 10, &[("landuse", "grass")]),
            // Bigger than all the paving, so it cannot open any of it.
            square(6, (0, 0), 60, &[("landuse", "meadow")]),
        ];

        let cache = FloodFillCache::precompute(&elements, None);
        let mut roads = RoadMaskBitmap::new(&xzbbox);
        roads.set(18, 15);
        let sealed = cache.collect_sealed_surfaces(&elements, &roads).unwrap();

        assert!(
            sealed.contains(30, 30),
            "the car park around the lawn stays paved"
        );
        assert!(
            !sealed.contains(17, 17),
            "the lawn inside the car park keeps its ground"
        );
        assert!(
            sealed.contains(13, 13),
            "paving inside the lawn is paved again"
        );
        assert!(
            sealed.contains(18, 15),
            "a road across the lawn stays sealed"
        );
        assert!(sealed.contains(65, 65), "a pitch keeps its whole footprint");
    }

    #[test]
    fn nothing_to_seal_shares_the_road_mask() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 99, 99).unwrap();
        let elements = vec![ProcessedElement::Way(tagged_way(
            9,
            &[(10, 10), (30, 10), (30, 25), (10, 25), (10, 10)],
            &[("landuse", "forest")],
        ))];

        let cache = FloodFillCache::precompute(&elements, None);
        let roads = RoadMaskBitmap::new(&xzbbox);

        assert!(
            cache.collect_sealed_surfaces(&elements, &roads).is_none(),
            "a second full-world bitmap must not be allocated for nothing"
        );
    }

    #[test]
    fn a_pitch_the_roads_already_own_shares_the_road_mask() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 99, 99).unwrap();
        let elements = vec![ProcessedElement::Way(tagged_way(
            11,
            &[(10, 10), (30, 10), (30, 25), (10, 25), (10, 10)],
            &[("leisure", "pitch")],
        ))];

        let cache = FloodFillCache::precompute(&elements, None);
        let mut roads = RoadMaskBitmap::new(&xzbbox);
        for x in 5..=35 {
            for z in 5..=30 {
                roads.set(x, z);
            }
        }

        assert!(
            cache.collect_sealed_surfaces(&elements, &roads).is_none(),
            "the pitch adds no column, so the road mask is already the answer"
        );
    }

    fn area_relation(id: u64, member_id: u64, tags: &[(&str, &str)]) -> ProcessedElement {
        let mut member = tagged_way(
            member_id,
            &[(0, 0), (20, 0), (20, 20), (0, 20), (0, 0)],
            &[],
        );
        member.tags.clear();
        ProcessedElement::Relation(crate::osm_parser::ProcessedRelation {
            id,
            members: vec![crate::osm_parser::ProcessedMember {
                role: ProcessedMemberRole::Outer,
                way: std::sync::Arc::new(member),
            }],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    #[test]
    fn filled_relations_are_precomputed_once_and_others_are_not() {
        let elements = vec![
            area_relation(1, 101, &[("natural", "sand")]),
            area_relation(2, 102, &[("leisure", "park")]),
            // Painted by nothing, so filling it up front would only cost memory.
            area_relation(3, 103, &[("landuse", "residential")]),
            area_relation(4, 104, &[("natural", "water")]),
            area_relation(5, 105, &[("building", "yes")]),
        ];
        let cache = FloodFillCache::precompute(&elements, None);
        assert!(cache.get_cached(101).is_some_and(|f| !f.is_empty()));
        assert!(cache.get_cached(102).is_some_and(|f| !f.is_empty()));
        for skipped in [103, 104, 105] {
            assert!(cache.get_cached(skipped).is_none(), "member {skipped}");
        }
    }
}
