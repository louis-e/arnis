mod args;
mod block_definitions;
mod bresenham;
mod colors;
mod data_processing;
mod element_processing;
mod floodfill;
mod osm_parser;
mod retrieve_data;
mod version_check;
mod world_editor;

use args::Args;
use clap::Parser;
use colored::*;
use std::fs::File;
use std::io::Write;

fn print_banner() {
    let version: &str = env!("CARGO_PKG_VERSION");
    let repository: &str = env!("CARGO_PKG_REPOSITORY");
    println!(
        r#"
        ▄████████    ▄████████ ███▄▄▄▄    ▄█     ▄████████
        ███    ███   ███    ███ ███▀▀▀██▄ ███    ███    ███
        ███    ███   ███    ███ ███   ███ ███▌   ███    █▀
        ███    ███  ▄███▄▄▄▄██▀ ███   ███ ███▌   ███
      ▀███████████ ▀▀███▀▀▀▀▀   ███   ███ ███▌ ▀███████████
        ███    ███ ▀███████████ ███   ███ ███           ███
        ███    ███   ███    ███ ███   ███ ███     ▄█    ███
        ███    █▀    ███    ███  ▀█   █▀  █▀    ▄████████▀
                     ███    ███

                          version {}
                {}
        "#,
        version,
        repository.bright_white().bold()
    );
}

fn main() {
    print_banner();

    // Check for updates
    if let Err(e) = version_check::check_for_updates() {
        eprintln!(
            "{}: {}",
            "Error checking for version updates".red().bold(),
            e
        );
    }

    // Parse input arguments
    let args: Args = Args::parse();
    args.run();

    let bbox: Vec<f64> = args
        .bbox
        .as_ref()
        .expect("Bounding box is required")
        .split(',')
        .map(|s: &str| s.parse::<f64>().expect("Invalid bbox coordinate"))
        .collect::<Vec<f64>>();

    let bbox_tuple: (f64, f64, f64, f64) = (bbox[0], bbox[1], bbox[2], bbox[3]);

    // Fetch data
    let raw_data: serde_json::Value =
        retrieve_data::fetch_data(bbox_tuple, args.file.as_deref(), args.debug, "requests")
            .expect("Failed to fetch data");

    // Parse raw data
    let (mut parsed_elements, scale_factor_x, scale_factor_z) =
        osm_parser::parse_osm_data(&raw_data, bbox_tuple, &args);
    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));

    // Write the parsed OSM data to a file for inspection
    if args.debug {
        let mut output_file: File =
            File::create("parsed_osm_data.txt").expect("Failed to create output file");
        for element in &parsed_elements {
            writeln!(
                output_file,
                "Element ID: {}, Type: {}, Tags: {:?}, Nodes: {:?}",
                element.id, element.r#type, element.tags, element.nodes
            )
            .expect("Failed to write to output file");
        }
    }

    // Generate world
    data_processing::generate_world(parsed_elements, &args, scale_factor_x, scale_factor_z);
}
