#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod block_definitions;
mod bresenham;
mod colors;
mod coordinate_system;
mod data_processing;
mod element_processing;
mod floodfill;
mod ground;
mod map_transformation;
mod osm_parser;
#[cfg(feature = "gui")]
mod progress;
mod retrieve_data;
#[cfg(test)]
mod test_utilities;
mod version_check;
mod world_editor;

use args::Args;
use clap::Parser;
use colored::*;
use std::{env, fs, io::Write};

#[cfg(feature = "gui")]
mod gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

fn run_cli() {
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

    // Fetch data
    let raw_data: serde_json::Value =
        retrieve_data::fetch_data(args.bbox, args.file.as_deref(), args.debug, "requests")
            .expect("Failed to fetch data");

    let mut ground = ground::generate_ground_data(&args);

    // Parse raw data
    let (mut parsed_elements, mut xzbbox) =
        osm_parser::parse_osm_data(&raw_data, args.bbox, args.scale, args.debug);
    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));

    // Write the parsed OSM data to a file for inspection
    if args.debug {
        let mut output_file: fs::File =
            fs::File::create("parsed_osm_data.txt").expect("Failed to create output file");
        for element in &parsed_elements {
            writeln!(
                output_file,
                "Element ID: {}, Type: {}, Tags: {:?}",
                element.id(),
                element.kind(),
                element.tags(),
            )
            .expect("Failed to write to output file");
        }
    }

    // Transform map (parsed_elements). Operations are defined in a json file
    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);

    // Generate world
    let _ = data_processing::generate_world(parsed_elements, xzbbox, ground, &args);
}

fn main() {
    // If on Windows, free and reattach to the parent console when using as a CLI tool
    // Either of these can fail, but if they do it is not an issue, so the return value is ignored
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = FreeConsole();
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    // Only run CLI mode if the user supplied args.
    #[cfg(feature = "gui")]
    {
        let gui_mode = std::env::args().len() == 1; // Just "arnis" with no args
        if gui_mode {
            gui::run_gui();
        }
    }

    run_cli();
}
