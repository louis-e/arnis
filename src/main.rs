#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod bedrock_block_map;
mod bench;
mod biome;
mod block_definitions;
mod bresenham;
mod clipping;
mod colors;
mod coordinate_system;
mod data_processing;
mod deterministic_rng;
mod element_processing;
mod elevation;
mod elevation_data;
mod floodfill;
mod floodfill_cache;
mod ground;
mod ground_generation;
mod land_cover;
mod land_cover_bridge_repair;
mod land_cover_osm_water_override;
mod luanti_block_map;
mod map_preview;
mod map_renderer;
mod map_transformation;
mod models_3d;
mod ore_generation;
mod osm_parser;
mod overture;
#[cfg(feature = "gui")]
mod progress;
mod projection;
mod retrieve_data;
#[cfg(feature = "gui")]
mod telemetry;
#[cfg(test)]
mod test_utilities;
mod tile;
mod version_check;
mod water_depth;
mod world_editor;
mod world_utils;

use args::Args;
use clap::Parser;
use colored::*;
use std::path::PathBuf;
use std::{env, fs, io::Write};

// mimalloc scales far better than the system allocator under the concurrent
// 4 KiB section-vec / hashmap churn of tile-parallel processing.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "gui")]
mod gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn emit_gui_progress_update_ex(_progress: f64, _message: &str, _streaming: bool) {}
    pub fn emit_map_preview_ready() {}
    pub fn emit_show_in_folder(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

fn run_cli() {
    // Configure thread pool with 90% CPU cap to keep system responsive
    floodfill_cache::configure_rayon_thread_pool(0.9);

    // Clean up old cached elevation tiles on startup
    elevation_data::cleanup_old_cached_tiles();

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

    // Fire-and-forget update check; prints a one-line notice on a background thread.
    version_check::check_for_updates_async();

    // Parse input arguments
    let args: Args = Args::parse();

    // Validate arguments (path requirements differ between Java and Bedrock)
    if let Err(e) = args::validate_args(&args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }

    // Heads-up for very large areas: generation is long and memory-heavy, and big
    // requests load the public OpenStreetMap / elevation servers. Non-blocking.
    {
        const MAX_RECOMMENDED_AREA_KM2: f64 = 250.0;
        let b = &args.bbox;
        let mid_lat = ((b.min().lat() + b.max().lat()) / 2.0).to_radians();
        let width_m = (b.max().lng() - b.min().lng()) * 111_320.0 * mid_lat.cos();
        let height_m = (b.max().lat() - b.min().lat()) * 111_320.0;
        let area_km2 = (width_m * height_m).abs() / 1_000_000.0;
        if area_km2 > MAX_RECOMMENDED_AREA_KM2 {
            eprintln!(
                "{} Large area selected (~{:.0} km²). Generation may take a long time and \
                 use many GB of memory, and places heavy load on public OpenStreetMap and \
                 elevation servers. Use a smaller area if this was unintended.",
                "Note:".yellow().bold(),
                area_km2
            );
        }
    }

    // Determine world format and output path
    let world_format = if args.bedrock {
        world_editor::WorldFormat::BedrockMcWorld
    } else if args.luanti {
        world_editor::WorldFormat::LuantiWorld
    } else {
        world_editor::WorldFormat::JavaAnvil
    };

    // Build the generation output path and level name
    let (generation_path, level_name) = if args.bedrock {
        // Bedrock: generate .mcworld file in user-specified path or Desktop
        let output_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory);
        let (output_path, lvl_name) = world_utils::build_bedrock_output(&args.bbox, output_dir);
        (output_path, Some(lvl_name))
    } else if args.luanti {
        let base_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_luanti_worlds_directory);
        let _ = std::fs::create_dir_all(&base_dir);
        let mut counter = 1;
        let world_name = loop {
            let candidate = format!("Arnis Luanti World {counter}");
            if !base_dir.join(&candidate).exists() {
                break candidate;
            }
            counter += 1;
        };
        let world_path = base_dir.join(&world_name);
        println!(
            "Creating Luanti world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        (world_path, Some(world_name))
    } else {
        // Java: create a new world in the provided output directory
        let base_dir = args.path.clone().unwrap();
        let world_path = match world_utils::create_new_world(&base_dir) {
            Ok(path) => PathBuf::from(path),
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                std::process::exit(1);
            }
        };
        println!(
            "Created new world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        if args.disable_height_limit {
            if let Err(e) = world_utils::install_tall_datapack(&world_path) {
                eprintln!(
                    "{} Failed to install tall-world datapack: {}",
                    "Error:".red().bold(),
                    e
                );
                std::process::exit(1);
            }
            eprintln!(
                "Note: tall-world datapack installed (requires Minecraft 1.21.4+). \
                 First load will prompt 'Experimental Features'; world can't be uploaded to Realms."
            );
        }
        (world_path, None)
    };

    // Top-level phase timer (active only under --benchmark). generate_world has
    // its own internal Bench for the block-placement phases.
    let mut bench = bench::Bench::new(args.benchmark);

    // Fetch data
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
    bench.mark("osm_fetch");

    // Fetch supplementary Overture Maps buildings right after the OSM download
    // (it only needs the bbox); the dedup against OSM runs after parsing below.
    println!("{} Fetching Overture Maps data...", "  [+]".bold());
    let overture_elements = overture::fetch_overture_buildings(&args.bbox, args.scale, args.debug);
    bench.mark("overture_fetch");

    let mut ground = ground::generate_ground_data(&args);
    bench.mark("terrain_total");

    // Parse raw data
    let (mut parsed_elements, mut xzbbox, outline_suppression) =
        osm_parser::parse_osm_data(raw_data, args.bbox, args.scale, args.debug, args.projection);
    bench.mark("parse_osm");

    // Merge the Overture buildings now that the OSM elements are parsed.
    if !overture_elements.is_empty() {
        let before_count = parsed_elements.len();
        let unique_overture =
            overture::deduplicate_against_osm(overture_elements, &parsed_elements);
        parsed_elements.extend(unique_overture);
        let added = parsed_elements.len() - before_count;
        println!(
            "  Added {} buildings from Overture Maps",
            added.to_string().bright_white().bold()
        );
    } else {
        println!("  No additional buildings from Overture Maps for this area");
    }

    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));
    bench.mark("sort_priority");

    // OSM water override first, then bridge repair handles remaining bridge-shadow cells.
    ground.apply_osm_water_override(&parsed_elements, &xzbbox);
    if args.debug {
        ground.save_land_cover_debug_image("landcover_debug_post_osm_water");
    }
    ground.apply_bridge_land_cover_repair(&parsed_elements, &xzbbox, args.scale);
    if args.debug {
        ground.save_land_cover_debug_image("landcover_debug_post_bridge_repair");
    }
    bench.mark("landcover_osm_repair");

    // Write the parsed OSM data to a file for inspection
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

    // Transform map (parsed_elements). Operations are defined in a json file
    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);
    bench.mark("transform_map");

    // Apply rotation if specified
    if args.rotation.abs() > f64::EPSILON {
        if let Err(e) = map_transformation::rotate::rotate_world(
            args.rotation,
            &mut parsed_elements,
            &mut xzbbox,
            &mut ground,
        ) {
            eprintln!("{} Rotation failed: {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    }

    // Convert spawn lat/lng to Minecraft XZ coordinates if provided
    let spawn_point: Option<(i32, i32)> = match (args.spawn_lat, args.spawn_lng) {
        (Some(lat), Some(lng)) => {
            use coordinate_system::geographic::LLPoint;
            use coordinate_system::transformation::CoordTransformer;

            let llpoint = LLPoint::new(lat, lng).unwrap_or_else(|e| {
                eprintln!("{} Invalid spawn coordinates: {}", "Error:".red().bold(), e);
                std::process::exit(1);
            });

            let (transformer, pre_rot_bbox) = match args.projection {
                projection::ProjectionKind::WebMercator => {
                    let origin_lat = (args.bbox.min().lat() + args.bbox.max().lat()) / 2.0;
                    let origin_lon = (args.bbox.min().lng() + args.bbox.max().lng()) / 2.0;
                    let proj =
                        projection::WebMercatorProjection::new(origin_lat, origin_lon, args.scale);
                    CoordTransformer::with_projection(&args.bbox, args.scale, &proj)
                }
                projection::ProjectionKind::Local => {
                    CoordTransformer::llbbox_to_xzbbox(&args.bbox, args.scale)
                }
            }
            .unwrap_or_else(|e| {
                eprintln!(
                    "{} Failed to convert spawn point: {}",
                    "Error:".red().bold(),
                    e
                );
                std::process::exit(1);
            });

            let xzpoint = transformer.transform_point(llpoint);
            let (sx, sz) = map_transformation::rotate::rotate_xz_point(
                xzpoint.x,
                xzpoint.z,
                args.rotation,
                &pre_rot_bbox,
            );

            Some((sx, sz))
        }
        _ => None,
    };

    // Derive terrain-aware spawn Y while `ground` is still in scope (it gets
    // moved into `generate_world_with_options` below). Used only for Java's
    // post-generation `set_spawn_in_level_dat` call — Bedrock derives spawn Y
    // independently inside `BedrockWriter::write_level_dat`.
    let spawn_y_for_java = spawn_point.map(|(sx, sz)| {
        use coordinate_system::cartesian::XZPoint;
        let rel = XZPoint::new(sx - xzbbox.min_x(), sz - xzbbox.min_z());
        ground.level(rel) + 3
    });

    // Build generation options
    let luanti_game = if args.luanti {
        Some(luanti_block_map::LuantiGame::Mineclonia)
    } else {
        None
    };

    let generation_options = data_processing::GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point,
        luanti_game,
        ground_level: args.ground_level,
    };

    // Generate world
    match data_processing::generate_world_with_options(
        parsed_elements,
        xzbbox,
        args.bbox,
        ground,
        &args,
        generation_options,
        outline_suppression,
    ) {
        Ok(_) => {
            if args.bedrock {
                println!(
                    "{} Bedrock world saved to: {}",
                    "Done!".green().bold(),
                    generation_path.display()
                );
            }

            // For Java Edition, update spawn point in level.dat if provided
            if !args.bedrock {
                if let (Some((spawn_x, spawn_z)), Some(spawn_y)) = (spawn_point, spawn_y_for_java) {
                    if let Err(e) = world_utils::set_spawn_in_level_dat(
                        &generation_path,
                        spawn_x,
                        spawn_y,
                        spawn_z,
                    ) {
                        eprintln!(
                            "{} Failed to set spawn point in level.dat: {}",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    }
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
