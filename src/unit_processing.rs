//! Per-unit processing logic for parallel world generation.
//!
//! This module contains the functions that process a single region unit,
//! generating all the elements within that unit's bounds.

use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, STONE};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::data_processing::MIN_Y;
use crate::element_processing::*;
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache};
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::parallel_processing::ProcessingUnit;
use crate::world_editor::{WorldEditor, WorldFormat};
use std::path::PathBuf;
use std::sync::Arc;

use crate::element_processing::highways::HighwayConnectivityMap;

/// Shared data for unit processing - passed by reference to each unit
#[allow(dead_code)]
pub struct SharedProcessingData {
    pub ground: Arc<Ground>,
    pub highway_connectivity: Arc<HighwayConnectivityMap>,
    pub building_footprints: Arc<BuildingFootprintBitmap>,
    pub floodfill_cache: Arc<FloodFillCache>,
    pub llbbox: LLBBox,
    pub world_dir: PathBuf,
    pub format: WorldFormat,
    pub level_name: Option<String>,
    pub terrain_enabled: bool,
    pub ground_level: i32,
    pub fill_ground: bool,
    pub interior: bool,
    pub roof: bool,
    pub debug: bool,
    pub timeout: Option<std::time::Duration>,
}

/// Process a single unit with element references (no cloning).
/// The caller is responsible for saving and dropping the editor to free memory.
pub fn process_unit_refs<'a>(
    unit: &ProcessingUnit,
    elements: &[&ProcessedElement],
    shared: &SharedProcessingData,
    unit_bbox: &'a XZBBox,
    args: &Args,
) -> WorldEditor<'a> {
    // Create a WorldEditor for just this unit's bounds
    let mut editor = WorldEditor::new_with_format_and_name(
        shared.world_dir.clone(),
        unit_bbox,
        shared.llbbox,
        shared.format,
        shared.level_name.clone(),
        None, // Spawn point not set per-unit
    );

    // Set ground reference for elevation-aware block placement
    editor.set_ground(Arc::clone(&shared.ground));

    // Process all elements for this unit
    for element in elements {
        process_element(
            &mut editor,
            element,
            &shared.highway_connectivity,
            &shared.floodfill_cache,
            &shared.building_footprints,
            unit_bbox,
            args,
        );
    }

    // Generate ground layer for this unit
    generate_ground_for_unit(&mut editor, unit, shared, args);

    editor
}

/// Process a single unit and return the WorldEditor with blocks placed.
/// The caller is responsible for saving and dropping the editor to free memory.
#[allow(dead_code)]
pub fn process_unit<'a>(
    unit: &ProcessingUnit,
    elements: &[ProcessedElement],
    shared: &SharedProcessingData,
    unit_bbox: &'a XZBBox,
    args: &Args,
) -> WorldEditor<'a> {
    // Create a WorldEditor for just this unit's bounds
    let mut editor = WorldEditor::new_with_format_and_name(
        shared.world_dir.clone(),
        unit_bbox,
        shared.llbbox,
        shared.format,
        shared.level_name.clone(),
        None, // Spawn point not set per-unit
    );

    // Set ground reference for elevation-aware block placement
    editor.set_ground(Arc::clone(&shared.ground));

    // Process all elements for this unit
    for element in elements {
        process_element(
            &mut editor,
            element,
            &shared.highway_connectivity,
            &shared.floodfill_cache,
            &shared.building_footprints,
            unit_bbox,
            args,
        );
    }

    // Generate ground layer for this unit
    generate_ground_for_unit(&mut editor, unit, shared, args);

    editor
}

/// Process a single element, dispatching to the appropriate generator
fn process_element(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    xzbbox: &XZBBox,
    args: &Args,
) {
    match element {
        ProcessedElement::Way(way) => {
            if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                buildings::generate_buildings(editor, way, args, None, flood_fill_cache);
            } else if way.tags.contains_key("highway") {
                highways::generate_highways(
                    editor,
                    element,
                    args,
                    highway_connectivity,
                    flood_fill_cache,
                );
            } else if way.tags.contains_key("landuse") {
                landuse::generate_landuse(
                    editor,
                    way,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if way.tags.contains_key("natural") {
                natural::generate_natural(
                    editor,
                    element,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if way.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, element, args, flood_fill_cache);
            } else if way.tags.contains_key("leisure") {
                leisure::generate_leisure(
                    editor,
                    way,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if way.tags.contains_key("barrier") {
                barriers::generate_barriers(editor, element);
            } else if let Some(val) = way.tags.get("waterway") {
                if val == "dock" {
                    water_areas::generate_water_area_from_way(editor, way, xzbbox);
                } else {
                    waterways::generate_waterways(editor, way);
                }
            } else if way.tags.contains_key("bridge") {
                // bridges::generate_bridges(editor, way, ground_level); // TODO FIX
            } else if way.tags.contains_key("railway") {
                railways::generate_railways(editor, way);
            } else if way.tags.contains_key("roller_coaster") {
                railways::generate_roller_coaster(editor, way);
            } else if way.tags.contains_key("aeroway") || way.tags.contains_key("area:aeroway") {
                highways::generate_aeroway(editor, way, args);
            } else if way.tags.get("service") == Some(&"siding".to_string()) {
                highways::generate_siding(editor, way);
            } else if way.tags.contains_key("man_made") {
                man_made::generate_man_made(editor, element, args);
            }
        }
        ProcessedElement::Node(node) => {
            if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                doors::generate_doors(editor, node);
            } else if node.tags.contains_key("natural")
                && node.tags.get("natural") == Some(&"tree".to_string())
            {
                natural::generate_natural(
                    editor,
                    element,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if node.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, element, args, flood_fill_cache);
            } else if node.tags.contains_key("barrier") {
                barriers::generate_barrier_nodes(editor, node);
            } else if node.tags.contains_key("highway") {
                highways::generate_highways(
                    editor,
                    element,
                    args,
                    highway_connectivity,
                    flood_fill_cache,
                );
            } else if node.tags.contains_key("tourism") {
                tourisms::generate_tourisms(editor, node);
            } else if node.tags.contains_key("man_made") {
                man_made::generate_man_made_nodes(editor, node);
            }
        }
        ProcessedElement::Relation(rel) => {
            if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                buildings::generate_building_from_relation(editor, rel, args, flood_fill_cache);
            } else if rel.tags.contains_key("water")
                || rel
                    .tags
                    .get("natural")
                    .map(|val| val == "water" || val == "bay")
                    .unwrap_or(false)
            {
                water_areas::generate_water_areas_from_relation(editor, rel, xzbbox);
            } else if rel.tags.contains_key("natural") {
                natural::generate_natural_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if rel.tags.contains_key("landuse") {
                landuse::generate_landuse_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if rel.tags.get("leisure") == Some(&"park".to_string()) {
                leisure::generate_leisure_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if rel.tags.contains_key("man_made") {
                man_made::generate_man_made(editor, element, args);
            }
        }
    }
}

/// Generate ground layer (grass, dirt, bedrock) for a unit
fn generate_ground_for_unit(
    editor: &mut WorldEditor,
    unit: &ProcessingUnit,
    shared: &SharedProcessingData,
    _args: &Args,
) {
    let terrain_enabled = shared.terrain_enabled;
    let ground_level = shared.ground_level;

    // Process chunk by chunk within this unit for cache locality
    let min_chunk_x = unit.min_x >> 4;
    let max_chunk_x = unit.max_x >> 4;
    let min_chunk_z = unit.min_z >> 4;
    let max_chunk_z = unit.max_z >> 4;

    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            // Calculate the block range for this chunk, clamped to unit bounds
            let chunk_min_x = (chunk_x << 4).max(unit.min_x);
            let chunk_max_x = ((chunk_x << 4) + 15).min(unit.max_x);
            let chunk_min_z = (chunk_z << 4).max(unit.min_z);
            let chunk_max_z = ((chunk_z << 4) + 15).min(unit.max_z);

            for x in chunk_min_x..=chunk_max_x {
                for z in chunk_min_z..=chunk_max_z {
                    let ground_y = if terrain_enabled {
                        editor.get_ground_level(x, z)
                    } else {
                        ground_level
                    };

                    // Add default dirt and grass layer if there isn't a stone layer already
                    if !editor.check_for_block_absolute(x, ground_y, z, Some(&[STONE]), None) {
                        editor.set_block_absolute(GRASS_BLOCK, x, ground_y, z, None, None);
                        editor.set_block_absolute(DIRT, x, ground_y - 1, z, None, None);
                        editor.set_block_absolute(DIRT, x, ground_y - 2, z, None, None);
                    }

                    // Fill underground with stone if enabled
                    if shared.fill_ground {
                        editor.fill_blocks_absolute(
                            STONE,
                            x,
                            MIN_Y + 1,
                            z,
                            x,
                            ground_y - 3,
                            z,
                            None,
                            None,
                        );
                    }

                    // Generate bedrock at MIN_Y
                    editor.set_block_absolute(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));
                }
            }
        }
    }
}
