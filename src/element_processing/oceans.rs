use geo::{LineString, Point, Polygon};
use rstar::{Envelope, RTree, AABB};
use std::path::PathBuf;
use std::time::Instant;

use crate::{
    args::Args,
    block_definitions::{AIR, BEDROCK, WATER},
    coordinate_system::{
        geographic::{LLBBox, LLPoint},
        transformation::CoordTransformer,
    },
    floodfill::flood_fill_area,
    ground::Ground,
    osm_parser::ProcessedElement,
    world_editor::WorldEditor,
};
use rayon::prelude::*;

/// Represents a water body polygon with its geographic bounds
#[derive(Debug, Clone)]
struct WaterPolygon {
    polygon: Polygon,
    bounds: AABB<[f64; 2]>,
}

impl rstar::RTreeObject for WaterPolygon {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.bounds
    }
}

impl rstar::PointDistance for WaterPolygon {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let _p = Point::new(point[0], point[1]);
        // Simple distance calculation - can be optimized if needed
        let center = self.bounds.center();
        let dx = center[0] - point[0];
        let dy = center[1] - point[1];
        dx * dx + dy * dy
    }
}

/// Loads water polygons from a shapefile and builds an R-tree for spatial queries
fn load_water_shapefile(path: &PathBuf, bbox: &LLBBox) -> Result<RTree<WaterPolygon>, String> {
    println!("Loading water shapefile from: {:?}", path);
    let start = Instant::now();

    let mut reader = shapefile::Reader::from_path(path)
        .map_err(|e| format!("Failed to read shapefile: {}", e))?;

    let mut water_polygons = Vec::new();
    let mut skipped = 0;
    let mut loaded = 0;

    // Expand bbox slightly to catch polygons that might intersect at edges
    let bbox_margin = 0.1; // degrees
    let min_lat = bbox.min().lat() - bbox_margin;
    let min_lng = bbox.min().lng() - bbox_margin;
    let max_lat = bbox.max().lat() + bbox_margin;
    let max_lng = bbox.max().lng() + bbox_margin;
    let expanded_bbox = LLBBox::new(min_lat, min_lng, max_lat, max_lng)
        .map_err(|e| format!("Failed to create expanded bbox: {}", e))?;

    for shape_record in reader.iter_shapes_and_records() {
        let (shape, _record) = shape_record
            .map_err(|e| format!("Failed to read shape record: {}", e))?;

        match shape {
            shapefile::Shape::Polygon(poly) => {
                // Convert shapefile polygon to geo polygon
                for ring in poly.rings() {
                    if ring.points().is_empty() {
                        continue;
                    }

                    // Check if polygon intersects with our bounding box
                    let mut min_x = f64::MAX;
                    let mut max_x = f64::MIN;
                    let mut min_y = f64::MAX;
                    let mut max_y = f64::MIN;

                    for point in ring.points() {
                        min_x = min_x.min(point.x);
                        max_x = max_x.max(point.x);
                        min_y = min_y.min(point.y);
                        max_y = max_y.max(point.y);
                    }

                    // Skip polygons that don't intersect with our expanded bbox
                    if max_x < expanded_bbox.min().lng()
                        || min_x > expanded_bbox.max().lng()
                        || max_y < expanded_bbox.min().lat()
                        || min_y > expanded_bbox.max().lat()
                    {
                        skipped += 1;
                        continue;
                    }

                    let coords: Vec<(f64, f64)> = ring
                        .points()
                        .iter()
                        .map(|p| (p.x, p.y))
                        .collect();

                    if coords.len() < 3 {
                        continue;
                    }

                    let line_string = LineString::from(coords);
                    let polygon = Polygon::new(line_string, vec![]);

                    let bounds = AABB::from_corners(
                        [min_x, min_y],
                        [max_x, max_y],
                    );

                    water_polygons.push(WaterPolygon { polygon, bounds });
                    loaded += 1;
                }
            }
            shapefile::Shape::Polyline(_polyline) => {
                // Some shapefiles might use polylines for water boundaries
                // We can skip these or handle them if needed
                skipped += 1;
            }
            _ => {
                skipped += 1;
            }
        }
    }

    println!(
        "Loaded {} water polygons in {:.2}s (skipped {} outside bbox)",
        loaded,
        start.elapsed().as_secs_f64(),
        skipped
    );

    if water_polygons.is_empty() {
        return Err("No water polygons found in shapefile within bounding box".to_string());
    }

    Ok(RTree::bulk_load(water_polygons))
}

/// Generates oceans from shapefile data
pub fn generate_oceans(
    editor: &mut WorldEditor,
    _elements: &[ProcessedElement],
    ground: &Ground,
    args: &Args,
) {
    let water_shapefile = match &args.water_shapefile {
        Some(path) => path,
        None => {
            println!("No water shapefile provided, skipping ocean generation");
            return;
        }
    };

    let bbox = &args.bbox;
    let (transformation, _xzbbox) = match CoordTransformer::llbbox_to_xzbbox(bbox, args.scale) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to create coordinate transformer: {}", e);
            return;
        }
    };

    // Load water polygons from shapefile
    let rtree = match load_water_shapefile(water_shapefile, bbox) {
        Ok(tree) => tree,
        Err(e) => {
            eprintln!("Failed to load water shapefile: {}", e);
            return;
        }
    };

    println!("Processing ocean polygons...");
    let start_time = Instant::now();

    // Query polygons that intersect with our bounding box
    let query_envelope = AABB::from_corners(
        [bbox.min().lng(), bbox.min().lat()],
        [bbox.max().lng(), bbox.max().lat()],
    );

    let intersecting_polygons: Vec<_> = rtree
        .locate_in_envelope_intersecting(&query_envelope)
        .collect();

    println!(
        "Found {} polygons intersecting with bounding box",
        intersecting_polygons.len()
    );

    if intersecting_polygons.is_empty() {
        println!("No water polygons found in the specified area");
        return;
    }

    // Convert geographic polygons to Minecraft coordinate polygons
    let mut mc_polygons: Vec<Polygon> = Vec::new();

    for water_poly in intersecting_polygons {
        let geo_poly = &water_poly.polygon;
        let exterior = geo_poly.exterior();

        let mc_coords: Vec<(f64, f64)> = exterior
            .points()
            .map(|p| {
                let ll_point = LLPoint::new(p.y(), p.x()).unwrap_or_else(|_| {
                    LLPoint::new(0.0, 0.0).unwrap()
                });
                let xz = transformation.transform_point(ll_point);
                (xz.x as f64, xz.z as f64)
            })
            .collect();

        if mc_coords.len() >= 3 {
            let mc_line_string = LineString::from(mc_coords);
            let mc_polygon = Polygon::new(mc_line_string, vec![]);
            mc_polygons.push(mc_polygon);
        }
    }

    if mc_polygons.is_empty() {
        println!("No valid polygons after coordinate transformation");
        return;
    }

    // Get world bounds
    let (min_x, min_z) = editor.get_min_coords();
    let (max_x, max_z) = editor.get_max_coords();

    println!(
        "Filling water in area: ({}, {}) to ({}, {})",
        min_x, min_z, max_x, max_z
    );

    // Calculate sea level once
    let sea_level_y = ground.sea_level();

    println!("Processing {} ocean polygons in parallel...", mc_polygons.len());

    // Process polygons in parallel to get all water coordinates
    let all_water_coords: Vec<(i32, i32)> = mc_polygons
        .par_iter()
        .flat_map(|mc_polygon| {
            // Convert polygon to coordinate list for flood_fill_area
            let coords: Vec<(i32, i32)> = mc_polygon
                .exterior()
                .points()
                .map(|p| (p.x() as i32, p.y() as i32))
                .collect();

            // Get filled coordinates using optimized flood fill (no timeout for oceans)
            flood_fill_area(&coords, None)
        })
        .collect();

    println!("Placing {} water blocks...", all_water_coords.len());

    // Place water blocks at all filled coordinates (sequential for thread safety)
    for (x, z) in all_water_coords {
        // Determine current terrain surface at this column
        let ground_y = if ground.elevation_enabled {
            editor.get_absolute_y(x, 0, z)
        } else {
            sea_level_y
        };

        // Always place a surface water layer up to sea level
        let min_water_y = (sea_level_y - 2).min(ground_y);
        for y in min_water_y..=sea_level_y {
            editor.set_block_absolute(WATER, x, y, z, None, Some(&[BEDROCK]));
        }

        // If terrain is above sea level inside polygon, carve it down (replace with air)
        if ground_y > sea_level_y {
            for y in (sea_level_y + 1)..=ground_y {
                editor.set_block_absolute(AIR, x, y, z, None, Some(&[BEDROCK]));
            }
        } else {
            // If terrain is below sea level, extend water down to terrain
            for y in ground_y..=(sea_level_y - 3).max(ground_y) {
                editor.set_block_absolute(WATER, x, y, z, None, Some(&[BEDROCK]));
            }
        }
    }

    println!(
        "Ocean generation completed in {:.2}s",
        start_time.elapsed().as_secs_f64()
    );
}

