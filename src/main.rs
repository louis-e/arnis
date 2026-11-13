#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod arnis_tiles;
mod bedrock_addon;
mod bedrock_block_map;
mod bench;
mod biome;
mod block_definitions;
mod block_palette;
mod bresenham;
mod building_facades;
mod canopy;
mod caves;
mod celestial;
mod climate;
mod climate_field;
mod clipping;
mod colors;
mod coordinate_system;
mod data_plan;
mod data_processing;
mod datapack;
mod decals;
mod deferred_placement;
mod deterministic_rng;
#[cfg(feature = "gui")]
mod discord;
mod distant_horizons;
mod ecoregion;
mod element_processing;
mod elevation;
mod elevation_data;
mod floodfill;
mod floodfill_cache;
mod geo_grid;
mod grid_ops;
mod ground;
mod ground_decoration;
mod ground_generation;
mod ground_scatter;
mod hytale_block_map;
mod land_cover;
mod landmarks;
mod live;
mod luanti_block_map;
mod map_item;
mod map_item_palette;
mod map_preview;
mod map_renderer;
mod map_transformation;
mod mapillary;
mod mode_filter;
mod models_3d;
mod net;
mod one_world;
mod ore_generation;
mod osm_parser;
mod osm_pbf;
mod osm_tiles;
mod output_args;
mod overture;
#[cfg(feature = "gui")]
mod preview_3d;
#[cfg(feature = "gui")]
mod progress;
mod progress_json;
mod projection;
mod retrieve_data;
mod river_bed;
mod scale;
mod structures;
#[cfg(feature = "gui")]
mod telemetry;
mod terrain_surface;
#[cfg(test)]
mod test_utilities;
mod tile;
mod transfer;
mod trees;
mod version_check;
mod voxy;
mod water_depth;
mod water_detail;
mod water_probe;
mod water_steps;
mod work_units;
mod world_editor;
mod world_utils;
mod y_bounds;

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
#[cfg(all(feature = "gui", any(target_os = "linux", all(test, unix))))]
mod linux_gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    /// Mirrors the real module's constant so callers outside the GUI feature
    /// still compile; nothing here reads it, the emits below do nothing.
    pub const MESSAGE_ONLY: f64 = -1.0;
    // `--progress json` is the one consumer left without a window.
    pub fn emit_gui_error(message: &str) {
        crate::progress_json::error(message);
    }
    pub fn emit_gui_progress_update(progress: f64, message: &str) {
        crate::progress_json::progress(progress, message);
    }
    pub fn emit_gui_progress_update_ex(progress: f64, message: &str, _streaming: bool) {
        crate::progress_json::progress(progress, message);
    }
    pub fn emit_gui_piece(_piece: &serde_json::Value) {}
    pub fn emit_gui_transfer(progress: f64, message: &str, transfer: &crate::transfer::Transfer) {
        crate::progress_json::progress(progress, message);
        crate::progress_json::record("transfer", serde_json::json!(transfer));
    }
    pub fn while_waiting<T>(_waiting_for: &str, wait: impl FnOnce() -> T) -> T {
        wait()
    }
    pub fn emit_map_preview_ready() {}
    pub fn emit_show_in_folder(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

/// Reattach to the console this process was launched from, so terminal output
/// works in both CLI and GUI runs.
///
/// Deliberately no `FreeConsole` first: a windows-subsystem process starts with
/// no console of its own, so `AttachConsole` alone reaches the parent terminal.
/// Freeing first can only invalidate the inherited handles, and a `FreeConsole`
/// that succeeds followed by a reattach that fails leaves stdout/stderr pointing
/// at a dead console, which turns every later `println!` into a panic.
#[cfg(target_os = "windows")]
fn attach_parent_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// The One World a CLI run holds, and the folder if the run created it.
struct OneWorldRun {
    lock: world_utils::SessionLock,
    created: Option<PathBuf>,
}

static ONE_WORLD_RUN: std::sync::Mutex<Option<OneWorldRun>> = std::sync::Mutex::new(None);

/// Releases the One World lock. On failure a world this run created is removed.
fn release_one_world(failed: bool) {
    let run = ONE_WORLD_RUN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(OneWorldRun { lock, created }) = run {
        drop(lock);
        if let (true, Some(dir)) = (failed, created) {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// From here on a failure keeps the world this run created: a job built in
/// pieces resumes from the pieces it finished.
fn keep_one_world() {
    if let Some(run) = ONE_WORLD_RUN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        run.created = None;
    }
}

/// `--offline` stops here rather than build a world with holes where the cache had nothing.
/// `fresh_world` is the folder this run made for it, removed again.
fn exit_if_offline_misses(fresh_world: Option<&std::path::Path>) {
    let missing = net::offline_misses();
    if missing.is_empty() {
        return;
    }
    eprintln!(
        "{} --offline, and the cache lacks data this area needs:",
        "Error:".red().bold()
    );
    for (what, refused) in missing {
        eprintln!("  - {what} ({refused} request(s) refused)");
    }
    eprintln!("Run the same command once with --prewarm in place of --offline to fetch it.");
    if let Some(dir) = fresh_world {
        let _ = fs::remove_dir_all(dir);
    }
    exit_failed();
}

/// `process::exit` skips destructors, so failures go through here.
fn exit_failed() -> ! {
    progress_json::error("generation failed, see stderr");
    release_one_world(true);
    std::process::exit(1);
}

/// `--dh-lod-only`: LODs from the regions of the One World at `world_dir`.
fn redraw_dh_lods(args: &mut Args, world_dir: &std::path::Path) -> Result<String, String> {
    // The world decides the build range its LODs are cut against.
    let manifest = one_world::Manifest::load(world_dir)?.ok_or_else(|| {
        format!(
            "{} is not a One World (no {}).",
            world_dir.display(),
            one_world::MANIFEST_FILE
        )
    })?;
    let _lock = world_utils::SessionLock::acquire(world_dir)
        .map_err(|_| "the world is open in Minecraft or being generated".to_string())?;
    args.disable_height_limit = manifest.disable_height_limit;
    args.height_multiplier = manifest.height_multiplier;
    world_editor::set_world_bounds(
        ground::extended_min_y_for(args),
        ground::world_top_y_for(args),
    );
    let s = distant_horizons::from_existing(world_dir).map_err(|e| e.to_string())?;
    let skipped = if s.skipped_regions + s.skipped_chunks > 0 {
        format!(
            " Skipped {} unreadable region file(s) and {} unreadable chunk(s).",
            s.skipped_regions, s.skipped_chunks
        )
    } else {
        String::new()
    };
    Ok(format!(
        "Distant Horizons LODs: {} sections written to {}.{skipped}",
        s.sections,
        world_dir.display()
    ))
}

/// The One World folder: `--world-name` inside the saves folder.
fn one_world_dir(args: &Args) -> PathBuf {
    let base_dir = args.path.clone().unwrap_or_else(|| {
        eprintln!(
            "{} --one-world needs --output-dir (the saves folder).",
            "Error:".red().bold()
        );
        exit_failed();
    });
    base_dir.join(one_world_name(args))
}

/// The One World's folder name: `--world-name`, else the default.
fn one_world_name(args: &Args) -> String {
    args.world_name
        .as_deref()
        .and_then(world_utils::world_folder_name)
        .unwrap_or_else(|| one_world::DEFAULT_WORLD_NAME.to_string())
}

fn run_cli() {
    // A live tile is one of many runs a live session starts, so it skips the greeting.
    let live_tile = std::env::args().any(|a| a == "--live-tile");

    // Parsed ahead of the banner so `--capabilities` prints its line alone.
    let mut args: Args = Args::parse();
    if args.capabilities {
        println!("{}", args::capabilities_json());
        return;
    }
    // Ahead of the early exits below, which would otherwise skip Live without a word.
    if let Err(e) = args::refuse_live_extras(&args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }
    if let Some(dir) = &args.export_tree_packs {
        match trees::pack_dir::export(dir).map(|(n, kept)| {
            let skipped = match kept {
                0 => String::new(),
                k => format!(" Skipped {k} already there, so your edits stay."),
            };
            let dir = dir.display();
            format!("{n} tree schematics exported to {dir}.{skipped}")
        }) {
            Ok(line) => println!("{line}"),
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(path) = &args.dump_loot_table {
        if let Err(e) = fs::write(
            path,
            element_processing::subprocessor::buildings_loot::BUILT_IN_JSON,
        ) {
            eprintln!(
                "{} Cannot write {}: {e}",
                "Error:".red().bold(),
                path.display()
            );
            std::process::exit(1);
        }
        return;
    }
    if let Some(path) = &args.loot_table {
        match element_processing::subprocessor::buildings_loot::load_loot_table(path) {
            Ok(table) => element_processing::subprocessor::buildings_loot::set_loot_table(table),
            Err(e) => {
                eprintln!(
                    "{} --loot-table {}: {e}",
                    "Error:".red().bold(),
                    path.display()
                );
                std::process::exit(1);
            }
        }
    }
    // Configure thread pool with 90% CPU cap to keep system responsive
    match args.thread_count() {
        Some(threads) => floodfill_cache::configure_rayon_threads(threads),
        None => floodfill_cache::configure_rayon_thread_pool(0.9),
    }
    if let Some(mb) = args.ram_budget_mb {
        data_processing::set_ram_budget_mb(mb);
    }
    if let Some(n) = args.max_downloads {
        net::set_max_requests(n as usize);
    }
    retrieve_data::set_overpass_urls(args.overpass_url.clone());
    net::set_offline(args.offline);
    net::set_prewarm(args.prewarm);

    // A plan is a dry run: it neither sweeps the tile cache nor makes the update request.
    let planning = args.units.plan_units.is_some();

    // A piece of a job is one of many runs too: no banner, update check or cache sweep.
    if !live_tile && args.units.one_world_unit.is_none() {
        print_banner(&args, planning);
    }

    if let Err(e) = args::resolve_shapes(&mut args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }
    let asked_mob_spawning = args.mob_spawning;
    if args.progress == Some(args::ProgressFormat::Json) {
        progress_json::enable();
    }
    args::apply_body_defaults(&mut args);
    args::apply_feature_needs(&mut args);
    y_bounds::apply(&mut args);
    let one_world = args.one_world;
    scale::budget::single_below_break_even(&mut args, one_world);

    // These two need no area, so they skip the general validation.
    if args.map_item_only || args.dh_lod_only {
        if let Err(e) = output_args::validate(&args) {
            eprintln!("{}: {}", "Error".red().bold(), e);
            exit_failed();
        }
        let world_dir = one_world_dir(&args);
        let result = if args.map_item_only {
            map_item::redraw_one_world_map(&world_dir)
                .map(|id| format!("World map item (map #{id}) redrawn over every area."))
        } else {
            redraw_dh_lods(&mut args, &world_dir)
        };
        match result {
            Ok(line) => println!("{line}"),
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
                exit_failed();
            }
        }
        return;
    }

    // Validate arguments (path requirements differ between Java and Bedrock)
    if let Err(e) = args::validate_args(&args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        exit_failed();
    }

    if args.climate_map.is_some() {
        if let Err(e) = climate_field::render(&args) {
            eprintln!("{}: {}", "Error".red().bold(), e);
            exit_failed();
        }
        return;
    }

    if args.live.live {
        run_live_cli(args);
        return;
    }
    run_generation_cli(args, asked_mob_spawning);
}

fn print_banner(args: &Args, planning: bool) {
    // Clean up old cached elevation tiles on startup; offline, the cache is all there is.
    if !args.no_cache_sweep && !planning && !args.offline {
        elevation_data::cleanup_old_cached_tiles();
    }

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
    if !args.no_update_check && !planning && !args.offline {
        version_check::check_for_updates_async();
    }
}

/// Plays the One World live on a local server until Ctrl+C.
fn run_live_cli(args: Args) {
    let fail = |msg: &str| -> ! {
        eprintln!("{} {msg}", "Error:".red().bold());
        std::process::exit(1);
    };
    let Some(saves_dir) = args.path.clone() else {
        fail("--live needs --output-dir (the Minecraft saves folder).");
    };
    let server_dir = live::server::server_dir();
    if !live::server::eula_accepted(&server_dir) {
        if !args.live.accept_eula {
            fail(&format!(
                "Running a Minecraft server needs your agreement to the Minecraft EULA ({}). Add --accept-eula to agree.",
                live::server::EULA_URL
            ));
        }
        if let Err(e) = live::server::accept_eula(&server_dir) {
            fail(&e);
        }
    }
    let world_name = one_world_name(&args);
    let start = match (args.spawn_lat, args.spawn_lng, args.bbox) {
        (Some(lat), Some(lng), _) => Some((lat, lng)),
        (_, _, Some(b)) => Some((
            (b.min().lat() + b.max().lat()) / 2.0,
            (b.min().lng() + b.max().lng()) / 2.0,
        )),
        _ => None,
    };
    let config = live::Config {
        saves_dir,
        world_name,
        start,
        version: args.live.version.clone(),
        view_distance: args.live.view_distance,
        port: args.live.port,
        messages: live::Messages::default(),
    };
    // Downloads are shown in tenths, so the terminal is not flooded.
    let last_download = std::sync::Mutex::new(None::<String>);
    let notify = std::sync::Arc::new(move |s: &live::Status| {
        let coarse = s.download.as_ref().map(|d| match d.text.rsplit_once(": ") {
            Some((label, _)) => {
                let tenth = d.progress.map_or(0, |p| (p * 10.0).floor() as u32 * 10);
                format!("{label}: {tenth}%")
            }
            None => d.text.clone(),
        });
        let mut last = last_download.lock().unwrap_or_else(|e| e.into_inner());
        if coarse != *last {
            if let Some(d) = &coarse {
                println!("[live] {d}");
            }
            *last = coarse;
        }
    });
    let session = std::sync::Arc::new(std::sync::Mutex::new(Some(live::Session::start(
        config, args, notify,
    ))));
    {
        let session = session.clone();
        let _ = ctrlc::set_handler(move || {
            println!("\n[live] Stopping, please wait while the world is saved...");
            if let Some(s) = session.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                s.request_stop();
            }
        });
    }
    loop {
        let finished = session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|s| s.is_finished());
        if finished {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let Some(session) = session.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    let status = session.status();
    session.stop_and_wait();
    if status.phase == "failed" {
        std::process::exit(1);
    }
}
fn run_generation_cli(mut args: Args, asked_mob_spawning: args::MobSpawning) {
    let started = std::time::Instant::now();
    // Cave zone-map mode renders the cave biome layout for --bbox and exits, before any world
    // exists. It uses the same zone picker, seed and --cave-biomes amounts as --caves.
    if args.cave_zone_map.is_some() {
        if let Err(e) = caves::zone_map::render(&args) {
            eprintln!("{}: {}", "Error".red().bold(), e);
            exit_failed();
        }
        return;
    }

    // Open up the world floor before anything touches the editor. The bundled packs already
    // grant the full engine range; without this the lower half of it goes unused. The ceiling
    // goes with it: chunk serialization needs the whole dimension span, not just the floor.
    world_editor::set_world_bounds(
        ground::extended_min_y_for(&args),
        ground::world_top_y_for(&args),
    );

    if args.legacy_terrain {
        eprintln!(
            "{} --terrain is deprecated: terrain is now on by default. \
             Use --mode geo-only for flat ground.",
            "Note:".yellow().bold()
        );
    }
    if asked_mob_spawning.spawns() && !args.mob_spawning.spawns() {
        eprintln!(
            "{} --mob-spawning is off: Zombie Outbreak and Disasters bring their own mobs, and \
             nothing lives off Earth.",
            "Note:".yellow().bold()
        );
    }
    // A run that loads no OSM data has nothing for the OSM in/out file args to act on.
    if args.skip_osm() && (args.file.is_some() || args.save_json_file.is_some()) {
        eprintln!(
            "{} No OpenStreetMap data is loaded below scale 0.3 or off Earth; --file/--save-json-file are ignored.",
            "Note:".yellow().bold()
        );
    }

    // Resolve the effective bounding box ONCE, up front, and thread the concrete value to every
    // consumer below (the CLI bbox is now optional). Precedence: explicit --bbox > file <bounds>
    // element > node-coordinate extent.
    //
    // A local .osm/.xml file is the only bbox source when --bbox is omitted, so it must be loaded
    // BEFORE the parallel fetch scope (Overture + land cover need the bbox) and before the bedrock
    // world-name / area-size steps below. The parsed data is kept in `preloaded_osm` so the file
    // isn't read twice. The Overpass and no-OSM paths already know the bbox from --bbox.
    let skip_objects = args.skip_objects();
    let skip_osm = args.skip_osm();
    let (mut preloaded_osm, mut effective_bbox) = match (skip_osm, args.file.as_deref()) {
        (false, Some(file)) => {
            let (data, file_bounds) = match retrieve_data::fetch_data_from_file(file) {
                Ok(loaded) => loaded,
                Err(e) => {
                    eprintln!("{} Failed to load OSM file: {}", "Error:".red().bold(), e);
                    exit_failed();
                }
            };
            // Whole ways reach past the downloaded area, and One World would overwrite
            // every chunk out to them.
            if args.one_world && args.bbox.is_none() && file_bounds.is_none() {
                eprintln!(
                    "{} '{file}' has no bounds; --one-world needs --bbox, or the area would grow to every node in the file.",
                    "Error:".red().bold()
                );
                exit_failed();
            }
            let bbox = match osm_parser::resolve_bbox(args.bbox, file_bounds, &data) {
                Some(bbox) => bbox,
                None => {
                    eprintln!(
                        "{} could not derive a bounding box from '{file}' (no <bounds> element and no usable node extent); pass --bbox explicitly.",
                        "Error:".red().bold()
                    );
                    exit_failed();
                }
            };
            // Report where a derived bbox came from (an explicit --bbox is echoed by the parse step).
            if args.bbox.is_none() {
                let source = if file_bounds.is_some() {
                    "the bounds in the file"
                } else {
                    "node coordinate extent"
                };
                println!(
                    "Derived bounding box from {source}: {},{},{},{}",
                    bbox.min().lat(),
                    bbox.min().lng(),
                    bbox.max().lat(),
                    bbox.max().lng()
                );
            }
            (Some(data), bbox)
        }
        _ => {
            // Overpass or no OSM at all: --bbox is required (enforced by validate_args above).
            let bbox = args.bbox.unwrap_or_else(|| {
                eprintln!(
                    "{} A bounding box is required. Provide --bbox, or --file with a local .osm/.xml file.",
                    "Error:".red().bold()
                );
                exit_failed();
            });
            (None, bbox)
        }
    };
    // validate_args checks the spawn only against an explicit --bbox.
    if let (None, Some(lat), Some(lng)) = (args.bbox, args.spawn_lat, args.spawn_lng) {
        let inside = coordinate_system::geographic::LLPoint::new(lat, lng)
            .is_ok_and(|p| effective_bbox.contains(&p));
        if !inside {
            eprintln!(
                "{} Spawn point (--spawn-lat, --spawn-lng) must be within the bounding box.",
                "Error:".red().bold()
            );
            exit_failed();
        }
    }

    // One World: snaps the bbox to the world's chunk grid and holds its lock.
    let mut one_world_paths: Option<PathBuf> = None;
    if args.one_world {
        let world_dir = one_world_dir(&args);
        if let Some(n) = args.units.plan_units {
            let planned = work_units::print_plan(&world_dir, &effective_bbox, &args, n);
            std::process::exit(planned.map_or_else(
                |e| {
                    eprintln!("{} {e}", "Error:".red().bold());
                    1
                },
                |()| 0,
            ));
        }
        if let Some(lease) = args.units.one_world_unit.clone() {
            // A piece of a job: the coordinator holds the world's lock.
            scale::child::watch_parent();
            effective_bbox =
                one_world::prepare_unit(&world_dir, &effective_bbox, &mut args, &lease)
                    .unwrap_or_else(|e| {
                        eprintln!("{} {}", "Error:".red().bold(), e);
                        exit_failed();
                    });
            world_editor::set_world_bounds(
                ground::extended_min_y_for(&args),
                ground::world_top_y_for(&args),
            );
            one_world_paths = Some(world_dir);
        } else {
            let session = one_world::prepare(&world_dir, &effective_bbox, &mut args)
                .unwrap_or_else(|e| {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    exit_failed();
                });
            effective_bbox = session.llbbox;
            // The world decides the build height.
            world_editor::set_world_bounds(
                ground::extended_min_y_for(&args),
                ground::world_top_y_for(&args),
            );
            *ONE_WORLD_RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some(OneWorldRun {
                lock: session.lock,
                created: session.created.then(|| world_dir.clone()),
            });
            if args.disable_height_limit && session.created {
                let (min_y, max_y) = world_utils::TALL_DATAPACK_FULL_RANGE;
                if let Err(e) = world_utils::install_tall_datapack(&world_dir, min_y, max_y) {
                    eprintln!(
                        "{} Failed to install tall-world datapack: {}",
                        "Error:".red().bold(),
                        e
                    );
                    exit_failed();
                }
            }
            one_world_paths = Some(world_dir);
        }
    }
    deterministic_rng::set_world_seed(args.seed.unwrap_or(0));
    if args.units.coordinates() {
        // A job built in pieces: this process only runs them, and each reads `--file` itself.
        drop(preloaded_osm.take());
        let world_dir = one_world_paths.clone().unwrap_or_default();
        match scale::run(&args, &world_dir) {
            Err(e) if e == scale::PARTIAL => println!("{e}"),
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                // A world with a finished piece in it stays for the resume; doubt keeps it too.
                if !matches!(one_world::Manifest::load(&world_dir), Ok(Some(m)) if m.areas.is_empty())
                {
                    keep_one_world();
                }
                exit_failed();
            }
            Ok(()) => {}
        }
        // A prewarm leaves no world it created behind.
        release_one_world(args.prewarm);
        progress_json::done(started);
        return;
    }
    let args = args;

    // Heads-up for very large areas: long, memory-heavy (about 6 GB here, the GUI's "very
    // large" tier) and heavy on the public elevation servers. Non-blocking.
    {
        const MAX_RECOMMENDED_AREA_KM2: f64 = 200.0;
        // area_km2 assumes Earth's radius, so a Moon bbox reads 13x too large.
        let r = args.body.scale_ratio();
        let area_km2 = effective_bbox.area_km2() * r * r;
        // Blocks, and with them memory, grow with the square of the world scale.
        let at_scale_1 = area_km2 * args.scale * args.scale;
        // Earth only: the coarse fixed scale makes a large area the normal case.
        if args.body.is_earth() && at_scale_1 > MAX_RECOMMENDED_AREA_KM2 {
            eprintln!(
                "{} Large area selected (~{:.0} km² at scale {:.2}). Generation may take \
                 several minutes, use many GB of memory and place heavy load on public \
                 elevation servers. Use a smaller area or scale if this was unintended.",
                "Note:".yellow().bold(),
                area_km2,
                args.scale
            );
        }
    }

    // Determine world format and output path
    let world_format = if args.bedrock {
        world_editor::WorldFormat::BedrockMcWorld
    } else if args.luanti {
        world_editor::WorldFormat::LuantiWorld
    } else if args.hytale {
        world_editor::WorldFormat::HytaleWorld
    } else {
        world_editor::WorldFormat::JavaAnvil
    };

    // Before the downloads and a fresh world, with the run's final scale and options; the
    // archive and Overpass both turn a box too large away. A One World this run made is
    // already there and goes again with exit_failed.
    let disk_dir = match (&one_world_paths, world_format) {
        (Some(dir), _) => dir.clone(),
        (None, world_editor::WorldFormat::BedrockMcWorld) => args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory),
        (None, world_editor::WorldFormat::LuantiWorld) => args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_luanti_worlds_directory),
        (None, world_editor::WorldFormat::HytaleWorld) => args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_hytale_saves_directory),
        (None, world_editor::WorldFormat::JavaAnvil) => args.path.clone().unwrap_or_default(),
    };
    if let Err(e) = world_utils::check_run_size(&args, &effective_bbox, world_format, &disk_dir) {
        eprintln!("{} {}", "Error:".red().bold(), e.detail);
        exit_failed();
    }

    // Build the generation output path and level name
    let (generation_path, level_name) = if args.mapillary_probe || args.prewarm {
        // The probe reports coverage and exits, and so does a prewarm, so neither may allocate
        // (and leave behind) an empty world directory on the way there.
        (PathBuf::new(), None)
    } else if args.bedrock {
        // Bedrock: generate .mcworld file in user-specified path or Desktop
        let output_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory);
        let (output_path, lvl_name) =
            world_utils::build_bedrock_output(&effective_bbox, output_dir, args.body);
        (output_path, Some(lvl_name))
    } else if args.sandbox() {
        let game = world_format.game();
        let (world_path, world_name) = world_utils::new_sandbox_world(&disk_dir, game);
        println!(
            "Creating {game} world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        (world_path, Some(world_name))
    } else if let Some(world_dir) = one_world_paths.clone() {
        (world_dir, None)
    } else {
        // Java: create a new world in the provided output directory
        let base_dir = args.path.clone().unwrap();
        let world_path = match world_utils::create_new_world(&base_dir) {
            Ok(path) => PathBuf::from(path),
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                exit_failed();
            }
        };
        println!(
            "Created new world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        (world_path, None)
    };

    // Top-level phase timer (active only under --benchmark). generate_world has
    // its own internal Bench for the block-placement phases.
    let mut bench = bench::Bench::new(args.benchmark);

    // A scale too small to render objects skips every object source: no Overpass query, no
    // Overture footprints. Terrain-only still loads OSM, for its water and nature.
    if args.skip_objects_due_to_scale() {
        println!(
            "{} Scale {:.2} is below {:.2}: skipping OpenStreetMap and Overture objects (terrain and land cover only)",
            "[1/7]".bold(),
            args.scale,
            args::OBJECT_SKIP_SCALE
        );
    } else if skip_osm {
        println!(
            "{} No OpenStreetMap data off Earth: terrain only",
            "[1/7]".bold()
        );
    } else if skip_objects {
        println!(
            "{} Terrain-only mode: keeping OpenStreetMap water and nature, no buildings or roads",
            "[1/7]".bold()
        );
    }

    // The Mapillary facade pipeline needs only the bbox too, and its downloads
    // are the longest thing in a run that uses it, so it starts here and is
    // collected inside `generate_world_with_options`, just before the buildings.
    let facade_job = if args.prewarm || net::offline() {
        if net::offline() && args.mapillary_pipeline_on() && !args.skip_objects() {
            println!("--offline: skipping Mapillary street-level imagery");
        }
        mapillary::FacadeJob::default()
    } else {
        mapillary::FacadeJob::start(&args, effective_bbox)
    };
    if facade_job.is_running() {
        println!(
            "{} Fetching Mapillary street-level imagery...",
            "  [+]".bold()
        );
    }

    // OSM, Overture and elevation/land-cover fetches only need the bbox, so run them in parallel.
    if args.overture && !skip_objects {
        println!("{} Fetching Overture Maps data...", "  [+]".bold());
    }
    let fetch_start = std::time::Instant::now();
    let (peak_tx, peaks) = crate::elevation::PeakSource::channel();
    let (raw_data, overture_data, mut ground) = std::thread::scope(|s| {
        // Owned here, so a panic below drops it and the terrain thread stops waiting.
        let peak_tx = peak_tx;
        let overture_handle = s.spawn(|| {
            let t = std::time::Instant::now();
            let data = if args.overture && !skip_objects {
                overture::fetch_overture_buildings(
                    &effective_bbox,
                    &projection::ProjectionSpec::from_args(&args),
                    args.overture_source,
                    args.debug,
                )
            } else {
                overture::OvertureData::default()
            };
            (data, t.elapsed())
        });
        let ground_handle = s.spawn(|| {
            let t = std::time::Instant::now();
            let ground = ground::generate_ground_data(&args, effective_bbox, &peaks);
            (ground, t.elapsed())
        });

        let t = std::time::Instant::now();
        // A local file was already parsed up front (to derive the bbox), so reuse that data.
        // A run without OSM carries no objects. Otherwise fetch from Overpass, in parallel with
        // the Overture and land-cover fetches spawned above.
        let raw_data = if skip_osm {
            osm_parser::OsmData::empty()
        } else if let Some(data) = preloaded_osm.take() {
            data
        } else {
            retrieve_data::fetch_osm_data(
                effective_bbox,
                args.scale,
                args.debug,
                args.downloader.as_str(),
                args.save_json_file.as_deref(),
                &args.osm_tiles_url,
                !args.no_tile_archive,
                osm_pbf::Source::from_args(&args).as_ref(),
            )
            .unwrap_or_else(|e| {
                // Offline, the other fetches finish first so the error lists all that is missing;
                // a failure that is not a cache miss (a bad .pbf or archive) still stops here.
                if net::offline() && !net::offline_misses().is_empty() {
                    return osm_parser::OsmData::empty();
                }
                eprintln!("{} Failed to fetch data: {e}", "Error:".red().bold());
                // A download may pass on another try; a local extract or a box too large will not.
                if osm_pbf::Source::from_args(&args).is_none()
                    && !net::offline()
                    && e.to_string() != retrieve_data::AREA_TOO_LARGE
                {
                    progress_json::set_error_kind(progress_json::NETWORK);
                }
                exit_failed();
            })
        };
        bench.report("osm_fetch", t.elapsed());
        // The terrain thread waits on this before fitting its height range. Overture's
        // building heights follow once it is in, for the thread to wait on only if needed.
        let _ = peak_tx.send(crate::elevation::TerrainHints::from_osm(&raw_data, &args));

        // A panicked worker already reported itself through the panic hook, so
        // degrade instead of taking the whole run down with it.
        let (overture_data, overture_dur) = overture_handle.join().unwrap_or_else(|_| {
            eprintln!(
                "{} Overture fetch failed, continuing without Overture buildings.",
                "Warning:".yellow().bold()
            );
            (overture::OvertureData::default(), std::time::Duration::ZERO)
        });
        bench.report("overture_fetch", overture_dur);
        if crate::elevation::TerrainHints::wants_overture(&args) {
            let _ = peak_tx.send(crate::elevation::TerrainHints::from_overture(
                &raw_data,
                &args,
                &overture_data,
            ));
        }
        // Nothing more follows, so a terrain thread still waiting is let go before the join.
        drop(peak_tx);
        let (ground, ground_dur) = ground_handle.join().unwrap_or_else(|_| {
            eprintln!("{} Terrain fetch failed.", "Error:".red().bold());
            exit_failed();
        });
        bench.report("terrain_total", ground_dur);

        (raw_data, overture_data, ground)
    });
    bench.report("fetch_total", fetch_start.elapsed());
    bench.reset();

    if args.prewarm {
        // 3D models download while they are placed, so their prescan runs here to fetch them.
        if args.use_3d && !skip_objects {
            let (elements, ..) = osm_parser::parse_osm_data(
                raw_data,
                effective_bbox,
                args.debug,
                &projection::ProjectionSpec::from_args(&args),
            );
            models_3d::Models3dPipeline::prescan(&elements, &args, &Default::default()).prewarm();
        }
        let uncached = net::uncached();
        if !uncached.is_empty() {
            eprintln!(
                "{} --prewarm could not cache everything this area needs:",
                "Error:".red().bold()
            );
            for what in uncached {
                eprintln!("  - {what}");
            }
            eprintln!("An --offline run of this area would stop on these.");
            exit_failed();
        }
        if let Some(root) = elevation::cache::user_cache_dir() {
            data_plan::refresh(&root, &args, effective_bbox);
        }
        println!(
            "{} Inputs for this area are cached; no world was written.",
            "Done!".green().bold()
        );
        // A prewarm leaves no world it created behind.
        release_one_world(true);
        progress_json::done(started);
        return;
    }
    let fresh_world = (world_format == world_editor::WorldFormat::JavaAnvil
        && args.one_world_run.is_none())
    .then_some(generation_path.as_path());
    exit_if_offline_misses(fresh_world);

    // The terrain fit has settled the build height.
    let mut args = args;
    if !args.mapillary_probe {
        let new_java_world = (world_format == world_editor::WorldFormat::JavaAnvil)
            .then_some(generation_path.as_path());
        if let Err(e) = ground::settle_build_height(
            &mut args,
            &ground,
            &effective_bbox,
            world_format,
            &disk_dir,
            new_java_world,
        ) {
            eprintln!("{} {}", "Error:".red().bold(), e);
            // The world made for this run holds nothing yet. A One World is released below.
            if let (Some(dir), None) = (new_java_world, &args.one_world_run) {
                let _ = fs::remove_dir_all(dir);
            }
            exit_failed();
        }
    }
    let args = args;

    // Parse raw data
    let (mut parsed_elements, mut xzbbox, outline_suppression, part_groups, housing_parts) =
        osm_parser::parse_osm_data(
            raw_data,
            effective_bbox,
            args.debug,
            &projection::ProjectionSpec::from_args(&args),
        );
    bench.mark("parse_osm");

    // Merge the Overture buildings now that the OSM elements are parsed.
    let overture::OvertureData {
        elements: overture_elements,
        hints: overture_hints,
        ..
    } = overture_data;

    // Fill height/levels on OSM buildings that have neither, before the
    // footprints are merged (Overture's own ways carry their tags already).
    let enriched = overture_hints.apply(&mut parsed_elements);
    if enriched > 0 {
        println!(
            "  Filled heights on {} OSM buildings from Overture Maps",
            enriched.to_string().bright_white().bold()
        );
    }

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
    } else if args.overture && !skip_objects && enriched == 0 {
        println!("  No additional buildings from Overture Maps for this area");
    }

    osm_parser::sort_by_priority(&mut parsed_elements);
    bench.mark("sort_priority");

    // OSM water override first, then bridge repair handles remaining bridge-shadow cells.
    ground.apply_osm_water_override(&parsed_elements, &xzbbox);
    // Before the land override, which would give them land to stand on.
    let wet =
        overture::drop_footprints_in_water(&mut parsed_elements, &ground, &xzbbox, args.scale);
    if wet > 0 {
        println!(
            "  Left out {} Overture buildings standing in water",
            wet.to_string().bright_white().bold()
        );
    }
    ground.apply_osm_land_override(&parsed_elements, &xzbbox, args.scale);
    if args.debug {
        ground.save_land_cover_debug_image("landcover_debug_post_osm_water");
    }
    ground.apply_bridge_land_cover_repair(&parsed_elements, &xzbbox, args.scale);
    // Last, once the water has stopped moving.
    ground.finish_land_cover(args.mode);
    if args.debug {
        ground.save_land_cover_debug_image("landcover_debug_post_bridge_repair");
    }
    bench.mark("landcover_osm_repair");

    // The overrides above still read roads and buildings as land; now the mode drops them.
    mode_filter::retain_for_mode(&mut parsed_elements, args.mode, args.buildings);
    let outline_suppression =
        mode_filter::outline_suppression_for_mode(outline_suppression, args.mode);

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

    // Cut to the drawn shapes while the frame is still the projection's own.
    xzbbox = projection::ProjectionSpec::from_args(&args)
        .shaped(&effective_bbox, xzbbox, args.shapes.as_ref())
        .unwrap_or_else(|e| {
            eprintln!("{} {}", "Error:".red().bold(), e);
            exit_failed();
        });

    // Transform map (parsed_elements). Operations are defined in a json file
    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);
    bench.mark("transform_map");

    // The default spawn is picked in the unrotated area and turned with it, as in the GUI.
    let pre_rotation_bbox = xzbbox.clone();

    // Apply rotation if specified
    if args.rotation.abs() > f64::EPSILON {
        if let Err(e) = map_transformation::rotate::rotate_world(
            args.rotation,
            &mut parsed_elements,
            &mut xzbbox,
            &mut ground,
        ) {
            eprintln!("{} Rotation failed: {}", "Error:".red().bold(), e);
            exit_failed();
        }
    }

    // Convert spawn lat/lng to Minecraft XZ coordinates if provided
    let spawn_point: Option<(i32, i32)> = match (args.spawn_lat, args.spawn_lng) {
        (Some(lat), Some(lng)) => {
            use coordinate_system::geographic::LLPoint;

            let llpoint = LLPoint::new(lat, lng).unwrap_or_else(|e| {
                eprintln!("{} Invalid spawn coordinates: {}", "Error:".red().bold(), e);
                exit_failed();
            });

            let (transformer, pre_rot_bbox) = projection::ProjectionSpec::from_args(&args)
                .transformer(&effective_bbox)
                .unwrap_or_else(|e| {
                    eprintln!(
                        "{} Failed to convert spawn point: {}",
                        "Error:".red().bold(),
                        e
                    );
                    exit_failed();
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
    //
    // Without a spawn given, Java starts at the corner of the area like the GUI does. The
    // level.dat template's own spot lies outside the area: beside it in a flat world, and
    // over nothing at all in a void one.
    // A piece of a job builds past its own area, so its corner is its own, not its halo's.
    let (spawn_x, spawn_z) = args
        .one_world_run
        .as_ref()
        .and_then(|r| r.unit.as_ref())
        .map_or_else(
            || pre_rotation_bbox.default_spawn(),
            |u| (u.rect[0] + 1, u.rect[1] + 1),
        );
    let java_spawn = spawn_point.or_else(|| {
        (world_format == world_editor::WorldFormat::JavaAnvil).then(|| {
            map_transformation::rotate::rotate_xz_point(
                spawn_x,
                spawn_z,
                args.rotation,
                &pre_rotation_bbox,
            )
        })
    });
    let spawn_y_for_java = java_spawn.map(|(sx, sz)| {
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

    // Probe mode stops here: it exists to answer "is this area covered?" before
    // anyone waits on a full generation.
    if args.mapillary_probe {
        let Some(token) = args.mapillary_token.as_deref().filter(|t| !t.is_empty()) else {
            eprintln!(
                "{} --mapillary-probe needs a token; pass --mapillary-token or set MAPILLARY_TOKEN.",
                "Error:".red().bold()
            );
            exit_failed();
        };
        let debug_dir = args.mapillary_debug_dir.clone();
        match mapillary::sample_area(
            &parsed_elements,
            &args,
            effective_bbox,
            token,
            debug_dir.is_some(),
        )
        .and_then(|report| mapillary::report(&report, debug_dir.as_deref()))
        {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("{} Mapillary probe failed: {e}", "Error:".red().bold());
                exit_failed();
            }
        }
    }

    // Generation places the map item at it; an extended One World moves it only to a marker.
    let extending = args.one_world_run.as_ref().is_some_and(|r| r.extending);
    let generation_options = data_processing::GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point: if extending { spawn_point } else { java_spawn },
        luanti_game,
        ground_level: args.ground_level,
        facades: facade_job,
    };

    // Generate world
    match data_processing::generate_world_with_options(
        parsed_elements,
        xzbbox,
        effective_bbox,
        ground,
        &args,
        generation_options,
        outline_suppression,
        part_groups,
        housing_parts,
    ) {
        Ok(_) => {
            if args.bedrock {
                println!(
                    "{} Bedrock world saved to: {}",
                    "Done!".green().bold(),
                    generation_path.display()
                );
            }

            // For Java Edition, write the spawn point into level.dat
            if world_format == world_editor::WorldFormat::JavaAnvil && !extending {
                if let (Some((spawn_x, spawn_z)), Some(spawn_y)) = (java_spawn, spawn_y_for_java) {
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
            // Generation stops after the 3D prescan when the cache lacks a model.
            exit_if_offline_misses(fresh_world);
            eprintln!("{} {}", "Error:".red().bold(), e);
            exit_failed();
        }
    }
    // A model the server lacks leaves a marker, not a miss, so this cannot drop a world for it.
    exit_if_offline_misses(fresh_world);
    release_one_world(false);
    progress_json::done(started);
}

fn main() {
    #[cfg(target_os = "windows")]
    attach_parent_console();

    // Only run CLI mode if the user supplied args.
    #[cfg(feature = "gui")]
    {
        let gui_mode = std::env::args().len() == 1; // Just "arnis" with no args
        if gui_mode {
            // WebKitGTK aborts on EGL before Rust hears of it, so Linux opens the
            // window from a child process it can start again with another renderer.
            #[cfg(target_os = "linux")]
            {
                if let Some(code) = linux_gui::supervise() {
                    std::process::exit(code);
                }
            }

            if let Err(e) = gui::run_gui() {
                eprintln!("{} {}", "Error:".red().bold(), e);
                // Tells the launcher this renderer failed, so it tries the next.
                #[cfg(target_os = "linux")]
                {
                    if linux_gui::current().is_some() && linux_gui::is_egl_error(&e) {
                        std::process::exit(linux_gui::EGL_FAILED_EXIT);
                    }
                }
                std::process::exit(1);
            }

            return;
        }
    }

    // Debug frames exceed the 1 MB Windows main stack.
    let cli = std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(run_cli)
        .expect("failed to start the CLI thread");
    if cli.join().is_err() {
        std::process::exit(101);
    }
}
