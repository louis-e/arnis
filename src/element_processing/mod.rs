pub mod advertising;
pub mod aerialways;
pub mod amenities;
pub mod attractions;
pub mod barriers;
pub mod blueprint;
pub mod bridge_modules;
pub mod bridge_styles;
pub mod bridge_towers;
pub mod bridges;
pub(crate) mod building_construction;
pub mod building_facade;
#[cfg(test)]
pub mod building_test_support;
pub mod buildings;
pub mod bush;
pub mod cars;
pub(crate) mod connected_blocks;
pub mod construction_site;
pub mod doors;
pub mod emergency;
pub mod field_texture;
pub mod fuel_stations;
pub mod highways;
pub mod historic;
pub mod landuse;
pub mod leisure;
pub mod man_made;
pub mod natural;
pub mod parking_garage;
pub mod power;
pub mod rail_track;
pub mod railways;
pub mod road_markings;
pub mod rooftops;
pub mod signage;
pub mod sport_pitches;
pub mod street_furniture;
pub(crate) mod structure_kit;
pub mod subprocessor;
mod surfaces;
pub mod swimming_pools;
pub mod tourisms;
pub mod traffic_lights;
pub mod train_bodies;
pub mod trains;
pub mod tree;
pub mod water_areas;
pub mod waterfront;
pub mod waterways;

use crate::floodfill_cache::RoadMaskBitmap;
use crate::osm_parser::ProcessedNode;
use std::collections::HashMap;

/// What `process_element` draws a relation as. Its relation arms, the ground-fill sort, the
/// fill cache and the parser all read this, so they cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationArea {
    Building,
    Water,
    Natural,
    Landuse,
    Park,
    ManMade,
    Pool,
}

impl RelationArea {
    pub fn of(tags: &HashMap<String, String>) -> Option<Self> {
        let natural = tags.get("natural").map(String::as_str);
        if tags.contains_key("building")
            || tags.contains_key("building:part")
            || tags.get("type").map(String::as_str) == Some("building")
        {
            Some(Self::Building)
        } else if swimming_pools::is_swimming_pool(tags) {
            Some(Self::Pool)
        } else if tags.contains_key("water") || matches!(natural, Some("water" | "bay")) {
            Some(Self::Water)
        } else if natural.is_some() {
            Some(Self::Natural)
        } else if tags.contains_key("landuse") {
            Some(Self::Landuse)
        } else if tags.get("leisure").map(String::as_str) == Some("park") {
            Some(Self::Park)
        } else if tags.contains_key("man_made") {
            Some(Self::ManMade)
        } else {
            None
        }
    }

    /// Whether the relation is drawn by filling each outer ring less its holes. Landuse
    /// that paints no ground draws nothing, and of man_made only piers, breakwaters and
    /// groynes are areas.
    pub fn fills_members(self, tags: &HashMap<String, String>) -> bool {
        match self {
            Self::Natural | Self::Park | Self::Pool => true,
            Self::Landuse => landuse_paints_ground(tags),
            Self::ManMade => matches!(
                tags.get("man_made").map(String::as_str),
                Some("pier" | "breakwater" | "groyne")
            ),
            Self::Building | Self::Water => false,
        }
    }

    /// Whether the relation draws its whole area, holes left out.
    pub fn draws_area(self, tags: &HashMap<String, String>) -> bool {
        self == Self::Water || self.fills_members(tags)
    }
}

/// `landuse` values that leave the ground to the land cover. They cover whole zones or grounds,
/// gardens, lawns and playing fields included, which the 10 m land cover resolves far better.
pub const LANDUSE_FROM_LAND_COVER: &[&str] =
    &["residential", "commercial", "education", "religious"];

/// Whether a `landuse` value paints ground at all.
/// `generate_landuse` bails on these before placing anything, and they are typically the largest
/// polygons around, so including them would push every real area behind unrelated elements.
pub fn landuse_paints_ground(tags: &HashMap<String, String>) -> bool {
    !tags
        .get("landuse")
        .is_some_and(|landuse| LANDUSE_FROM_LAND_COVER.contains(&landuse.as_str()))
}

/// Merges way segments that share endpoints into closed rings.
/// Used by water_areas.rs and boundaries.rs for assembling relation members.
pub fn merge_way_segments(rings: &mut Vec<Vec<ProcessedNode>>) {
    // Mark merged rings by index.  The previous implementation kept a `Vec<usize>`
    // and called `contains` from the nested matching loop.  For a relation with
    // many segments that adds another O(n) scan to the O(n²) pair search, making
    // ring assembly effectively cubic.  A boolean marker keeps the same merge
    // order while making the bookkeeping O(1).
    let mut removed = vec![false; rings.len()];
    let mut merged: Vec<Vec<ProcessedNode>> = vec![];

    // Match nodes by ID or proximity (handles synthetic nodes from bbox clipping)
    let nodes_match = |a: &ProcessedNode, b: &ProcessedNode| -> bool {
        if a.id == b.id {
            return true;
        }
        let dx = (a.x - b.x).abs();
        let dz = (a.z - b.z).abs();
        dx <= 1 && dz <= 1
    };

    for i in 0..rings.len() {
        for j in 0..rings.len() {
            if i == j {
                continue;
            }

            if removed[i] || removed[j] {
                continue;
            }

            let x: &Vec<ProcessedNode> = &rings[i];
            let y: &Vec<ProcessedNode> = &rings[j];

            // Skip empty rings (can happen after clipping)
            if x.is_empty() || y.is_empty() {
                continue;
            }

            let x_first = &x[0];
            let x_last = x.last().unwrap();
            let y_first = &y[0];
            let y_last = y.last().unwrap();

            // Skip already-closed rings
            if nodes_match(x_first, x_last) {
                continue;
            }

            if nodes_match(y_first, y_last) {
                continue;
            }

            if nodes_match(x_first, y_first) {
                removed[i] = true;
                removed[j] = true;

                let mut x: Vec<ProcessedNode> = x.clone();
                x.reverse();
                x.extend(y.iter().skip(1).cloned());
                merged.push(x);
            } else if nodes_match(x_last, y_last) {
                removed[i] = true;
                removed[j] = true;

                let mut x: Vec<ProcessedNode> = x.clone();
                x.extend(y.iter().rev().skip(1).cloned());

                merged.push(x);
            } else if nodes_match(x_first, y_last) {
                removed[i] = true;
                removed[j] = true;

                let mut y: Vec<ProcessedNode> = y.clone();
                y.extend(x.iter().skip(1).cloned());

                merged.push(y);
            } else if nodes_match(x_last, y_first) {
                removed[i] = true;
                removed[j] = true;

                let mut x: Vec<ProcessedNode> = x.clone();
                x.extend(y.iter().skip(1).cloned());

                merged.push(x);
            }
        }
    }

    for r in (0..removed.len()).rev() {
        if removed[r] {
            rings.remove(r);
        }
    }

    let merged_len: usize = merged.len();
    for m in merged {
        rings.push(m);
    }

    if merged_len > 0 {
        merge_way_segments(rings);
    }
}

/// Searches outward from (x, z) in the four cardinal directions and four
/// diagonals stepping by 2 up to max_radius blocks away, and returns the
/// (x, z) position of the nearest block that satisfies predicate
///
/// Returns None if no matching block is found within range.
fn get_nearest_block_matching(
    x: i32,
    z: i32,
    max_radius: i32,
    road_mask: &RoadMaskBitmap,
    predicate: impl Fn(bool) -> bool,
) -> Option<(i32, i32)> {
    for dist in (2..=max_radius).step_by(2) {
        let candidates = [
            (x, z - dist),
            (x, z + dist),
            (x - dist, z),
            (x + dist, z),
            (x - dist, z - dist),
            (x + dist, z + dist),
            (x - dist, z + dist),
            (x + dist, z - dist),
        ];
        for (cx, cz) in candidates {
            if predicate(road_mask.contains(cx, cz)) {
                return Some((cx, cz));
            }
        }
    }
    None
}

pub fn get_nearest_road_block(
    x: i32,
    z: i32,
    max_radius: i32,
    road_mask: &RoadMaskBitmap,
) -> Option<(i32, i32)> {
    get_nearest_block_matching(x, z, max_radius, road_mask, |on_road| on_road)
}

pub fn get_nearest_non_road_block(
    x: i32,
    z: i32,
    max_radius: i32,
    road_mask: &RoadMaskBitmap,
) -> Option<(i32, i32)> {
    get_nearest_block_matching(x, z, max_radius, road_mask, |on_road| !on_road)
}
