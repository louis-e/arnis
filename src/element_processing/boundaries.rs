//! Processing of administrative and urban boundaries.
//!
//! This module handles boundary elements that define urban areas (cities, boroughs, etc.)
//! and sets appropriate ground blocks for them.
//!
//! Boundaries are processed BEFORE landuse elements, so more specific landuse areas
//! (parks, residential, etc.) can override the general urban ground.

use crate::args::Args;
use crate::block_definitions::*;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
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
) {
    // Check if this is an urban boundary
    if !is_urban_boundary(&rel.tags) {
        return;
    }

    // Generate individual ways with their original tags
    for member in &rel.members {
        if member.role == ProcessedMemberRole::Outer {
            generate_boundary(editor, &member.way.clone(), args, flood_fill_cache);
        }
    }

    // Combine all outer ways into one with relation tags
    let mut combined_nodes = Vec::new();
    for member in &rel.members {
        if member.role == ProcessedMemberRole::Outer {
            combined_nodes.extend(member.way.nodes.clone());
        }
    }

    // Only process if we have nodes
    if !combined_nodes.is_empty() {
        // Create combined way with relation tags
        let combined_way = ProcessedWay {
            id: rel.id,
            nodes: combined_nodes,
            tags: rel.tags.clone(),
        };

        // Generate boundary area from combined way
        generate_boundary(editor, &combined_way, args, flood_fill_cache);
    }
}
