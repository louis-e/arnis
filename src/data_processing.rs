use crate::args::Args;
use crate::element_processing::*;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::get;
use std::fs;
use std::io::Write;
use std::path::Path;

pub fn generate_world(
    elements: Vec<ProcessedElement>,
    args: &Args,
    scale_factor_x: f64,
    scale_factor_z: f64,
) {
    println!("{} Processing data...", "[3/5]".bold());

    let region_template_path: &str = "region.template";
    let region_dir: String = format!("{}/region", args.path);
    let ground_level: i32 = -62;

    // Check if the region.template file exists, and download if necessary
    if !Path::new(region_template_path).exists() {
        let _ = download_region_template(region_template_path);
    }

    let mut editor: WorldEditor = WorldEditor::new(
        region_template_path,
        &region_dir,
        scale_factor_x,
        scale_factor_z,
        args,
    );

    // Process data
    let process_pb: ProgressBar = ProgressBar::new(elements.len() as u64);
    process_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
        .unwrap()
        .progress_chars("█▓░"));

    for element in &elements {
        process_pb.inc(1);

        if args.debug {
            process_pb.set_message(format!(
                "(Element ID: {} / Type: {})",
                element.id(),
                element.kind()
            ));
        } else {
            process_pb.set_message("");
        }

        match element {
            ProcessedElement::Way(way) => {
                if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                    buildings::generate_buildings(
                        &mut editor,
                        way,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if way.tags.contains_key("highway") {
                    highways::generate_highways(
                        &mut editor,
                        element,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if way.tags.contains_key("landuse") {
                    landuse::generate_landuse(
                        &mut editor,
                        way,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if way.tags.contains_key("natural") {
                    natural::generate_natural(
                        &mut editor,
                        element,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if way.tags.contains_key("amenity") {
                    amenities::generate_amenities(
                        &mut editor,
                        element,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if way.tags.contains_key("leisure") {
                    leisure::generate_leisure(
                        &mut editor,
                        way,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if way.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element, ground_level);
                } else if way.tags.contains_key("waterway") {
                    waterways::generate_waterways(&mut editor, way, ground_level);
                } else if way.tags.contains_key("bridge") {
                    bridges::generate_bridges(&mut editor, way, ground_level);
                } else if way.tags.contains_key("railway") {
                    railways::generate_railways(&mut editor, way, ground_level);
                } else if way.tags.get("service") == Some(&"siding".to_string()) {
                    highways::generate_siding(&mut editor, way, ground_level);
                }
            }
            ProcessedElement::Node(node) => {
                if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                    doors::generate_doors(&mut editor, node, ground_level);
                } else if node.tags.contains_key("natural")
                    && node.tags.get("natural") == Some(&"tree".to_string())
                {
                    natural::generate_natural(
                        &mut editor,
                        element,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if node.tags.contains_key("amenity") {
                    amenities::generate_amenities(
                        &mut editor,
                        element,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if node.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element, ground_level);
                } else if node.tags.contains_key("highway") {
                    highways::generate_highways(
                        &mut editor,
                        element,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                } else if node.tags.contains_key("tourism") {
                    tourisms::generate_tourisms(&mut editor, node, ground_level);
                }
            }
            ProcessedElement::Relation(rel) => {
                if rel.tags.contains_key("water") {
                    water_areas::generate_water_areas(
                        &mut editor,
                        rel,
                        ground_level,
                        args.timeout.as_ref(),
                    );
                }
            }
        }
    }

    process_pb.finish();

    // Generate ground layer
    let total_blocks: u64 = (scale_factor_x as i32 + 1) as u64 * (scale_factor_z as i32 + 1) as u64;
    let desired_updates: u64 = 1500;
    let batch_size: u64 = (total_blocks / desired_updates).max(1);

    let mut block_counter: u64 = 0;

    println!("Generating ground layer... {}", "[4/5]".bold());
    let ground_pb: ProgressBar = ProgressBar::new(total_blocks);
    ground_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} blocks ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    for x in 0..=(scale_factor_x as i32) {
        for z in 0..=(scale_factor_z as i32) {
            editor.set_block(
                &crate::block_definitions::GRASS_BLOCK,
                x,
                ground_level,
                z,
                None,
                None,
            );
            editor.set_block(
                &crate::block_definitions::DIRT,
                x,
                ground_level - 1,
                z,
                None,
                None,
            );

            block_counter += 1;
            if block_counter % batch_size == 0 {
                ground_pb.inc(batch_size);
            }
        }
    }

    ground_pb.inc(block_counter % batch_size);
    ground_pb.finish();

    // Save world
    editor.save();

    println!("{}", "Done! World generation complete.".green().bold());
}

/// Downloads the region template file from a remote URL and saves it locally.
fn download_region_template(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let url = "https://github.com/louis-e/arnis/raw/refs/heads/main/region.template";

    // Download the file
    let response = get(url)?;
    if !response.status().is_success() {
        return Err(format!("Failed to download file: HTTP {}", response.status()).into());
    }

    // Write the file to the specified path
    let mut file = fs::File::create(file_path)?;
    file.write_all(&response.bytes()?)?;

    Ok(())
}
