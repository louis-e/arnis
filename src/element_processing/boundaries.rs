//! Processing of administrative and urban boundaries.
//!
//! This module handles boundary elements that define urban areas (cities, boroughs, etc.)
//! and sets appropriate ground blocks for them.
//!
//! Boundaries are processed last but only fill empty areas, allowing more specific
//! landuse areas (parks, residential, etc.) to take priority over the general
//! urban ground.

use crate::args::Args;
use crate::block_definitions::*;
use crate::clipping::clip_way_to_bbox;
use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;

/// Checks if a boundary element represents an urban area that should have stone ground.
///
/// Returns true for:
/// - `boundary=administrative` with `admin_level >= 8` (city/borough level or smaller)
/// - `boundary=low_emission_zone` (urban traffic zones)
/// - `boundary=limited_traffic_zone` (urban traffic zones)
/// - `boundary=special_economic_zone` (developed industrial/commercial zones)
/// - `boundary=political` (electoral districts, usually urban)
fn is_urban_boundary(tags: &std::collections::HashMap<String, String>) -> bool {
    let Some(boundary_value) = tags.get("boundary") else {
        return false;
    };

    match boundary_value.as_str() {
        "administrative" => {
            // Only consider city-level or smaller (admin_level >= 8)
            // admin_level 2 = country, 4 = state, 6 = county, 8 = city/municipality
            if let Some(admin_level_str) = tags.get("admin_level") {
                if let Ok(admin_level) = admin_level_str.parse::<u8>() {
                    return admin_level >= 8;
                }
            }
            false
        }
        // Urban zones that should have stone ground
        "low_emission_zone" | "limited_traffic_zone" | "special_economic_zone" | "political" => {
            true
        }
        // Natural/protected areas should keep grass - don't process these
        // "national_park" | "protected_area" | "forest" | "forest_compartment" | "aboriginal_lands"
        // Statistical/administrative-only boundaries - don't affect ground
        // "census" | "statistical" | "postal_code" | "timezone" | "disputed" | "maritime" | etc.
        _ => false,
    }
}

/// Generate ground blocks for an urban boundary way.
pub fn generate_boundary(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    // Check if this is an urban boundary
    if !is_urban_boundary(&element.tags) {
        return;
    }

    // Get the area of the boundary element using cache
    let floor_area: Vec<(i32, i32)> =
        flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

    // Fill the area with smooth stone as ground block
    // Use None, None to only set where no block exists yet - don't overwrite anything
    for (x, z) in floor_area {
        editor.set_block(SMOOTH_STONE, x, 0, z, None, None);
    }
}

/// Generate ground blocks for an urban boundary relation.
pub fn generate_boundary_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    xzbbox: &XZBBox,
) {
    // Check if this is an urban boundary
    if !is_urban_boundary(&rel.tags) {
        return;
    }

    // Collect outer ways (unclipped) for merging
    let mut outers: Vec<Vec<ProcessedNode>> = rel
        .members
        .iter()
        .filter(|m| m.role == ProcessedMemberRole::Outer)
        .map(|m| m.way.nodes.clone())
        .collect();

    if outers.is_empty() {
        return;
    }

    // Merge way segments into closed rings
    super::merge_way_segments(&mut outers);

    // Clip each merged ring to bbox and process
    for ring in outers {
        if ring.len() < 3 {
            continue;
        }

        // Clip the merged ring to bbox
        let clipped_nodes = clip_way_to_bbox(&ring, xzbbox);
        if clipped_nodes.len() < 3 {
            continue;
        }

        // Create a ProcessedWay for the clipped ring
        let clipped_way = ProcessedWay {
            id: rel.id,
            nodes: clipped_nodes,
            tags: rel.tags.clone(),
        };

        // Generate boundary area from clipped way
        generate_boundary(editor, &clipped_way, args, flood_fill_cache);
    }
}
