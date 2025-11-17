#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "metrics")]
use arnis_core::metrics::MetricsRecorder;
use arnis_core::{
    data_processing, ground, map_transformation, osm_parser, retrieve_data, version_check, Args,
    PerformanceConfig,
};
use clap::Parser;
use colored::*;
use rayon::ThreadPoolBuilder;
use std::{env, fs, io::Write};

#[cfg(feature = "gui")]
use arnis_core::gui;

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

    if let Err(e) = version_check::check_for_updates() {
        eprintln!(
            "{}: {}",
            "Error checking for version updates".red().bold(),
            e
        );
    }

    let args: Args = Args::parse();

    let raw_data = match &args.file {
        Some(file) => retrieve_data::fetch_data_from_file(file),
        None => retrieve_data::fetch_data_from_overpass(
            args.bbox,
            args.debug,
            args.downloader.as_str(),
            args.save_json_file.as_deref(),
        ),
    }
    .expect("Failed to fetch data");

    let mut ground = ground::generate_ground_data(&args);

    let (mut parsed_elements, mut xzbbox) =
        osm_parser::parse_osm_data(raw_data, args.bbox, args.scale, args.debug);
    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));

    if args.debug {
        let mut buf = std::io::BufWriter::new(
            fs::File::create("parsed_osm_data.txt").expect("Failed to create output file"),
        );
        for element in &parsed_elements {
            writeln!(
                buf,
                "Element ID: {}, Type: {}, Tags: {:?}",
                element.id(),
                element.kind(),
                element.tags(),
            )
            .expect("Failed to write to output file");
        }
    }

    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);
    let _ = data_processing::generate_world(parsed_elements, xzbbox, args.bbox, ground, &args);

    #[cfg(feature = "metrics")]
    if let Some(metrics_out) = &args.metrics_out {
        let mut recorder = MetricsRecorder::new();
        if let Err(err) = recorder.write_to_path(metrics_out) {
            eprintln!("{}: {}", "Failed to write metrics".red().bold(), err);
        } else {
            println!("Metrics written to {}", metrics_out.display());
        }
    }
}

fn main() {
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = FreeConsole();
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    #[cfg(feature = "gui")]
    {
        let gui_mode = std::env::args().len() == 1;
        if gui_mode {
            gui::run_gui();
        }
    }

    let perf = PerformanceConfig::init_default();
    perf.log_config();
    ThreadPoolBuilder::new()
        .num_threads(perf.effective_threads)
        .build_global()
        .ok();

    run_cli();
}
