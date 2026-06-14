use crate::args::Args;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::*;
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};
use crate::ground::Ground;
use crate::ground_generation;
use crate::osm_parser::{OutlineSuppression, ProcessedElement};
use crate::progress::{emit_gui_progress_update, emit_gui_progress_update_ex, emit_show_in_folder};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use crate::tile;
use crate::world_editor::{FlushWorker, WorldEditor, WorldFormat};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Generation options that can be passed separately from CLI Args
#[derive(Clone)]
pub struct GenerationOptions {
    pub path: PathBuf,
    pub format: WorldFormat,
    pub level_name: Option<String>,
    pub spawn_point: Option<(i32, i32)>,
    pub luanti_game: Option<crate::luanti_block_map::LuantiGame>,
    pub ground_level: i32,
}

/// Process a single element by dispatching to the appropriate element processor.
///
/// Extracted from the main loop so the same dispatch runs in both the sequential
/// and the parallel tile-based processing paths. Every shared input is an
/// immutable reference (safe to share across rayon tile threads); the only
/// mutable state is the per-tile `editor` and `subway_points`.
///
/// Element suppression (3D-model / building-outline) and flood-fill cache
/// eviction are handled by the caller; the cache is shared immutably in the
/// parallel path and must not be mutated here.
#[allow(clippy::too_many_arguments)]
fn process_element(
    editor: &mut WorldEditor<'_>,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &highways::HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &CoordinateBitmap,
    building_passages: &CoordinateBitmap,
    road_mask: &CoordinateBitmap,
    xzbbox: &XZBBox,
    big_water_field: &crate::water_depth::BigWaterField,
    bridge_structures: &bridges::BridgeStructureMap,
    bridge_surface: &bridges::BridgeSurfaceMap,
    bridge_outlines: &bridge_styles::BridgeOutlineIndex,
    rail_bridge_internal_endpoints: &railways::RailBridgeInternalEndpoints,
    subway_points: &mut Vec<(i32, i32)>,
) {
    match element {
        ProcessedElement::Way(way) => {
            if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                buildings::generate_buildings(
                    editor,
                    way,
                    args,
                    None,
                    None,
                    flood_fill_cache,
                    building_passages,
                );
            } else if way.tags.contains_key("highway") {
                highways::generate_highways(
                    editor,
                    element,
                    args,
                    highway_connectivity,
                    flood_fill_cache,
                    road_mask,
                    bridge_structures,
                    bridge_surface,
                );
            } else if way.tags.contains_key("landuse") {
                landuse::generate_landuse(editor, way, args, flood_fill_cache, building_footprints);
            } else if way.tags.contains_key("natural") {
                natural::generate_natural(
                    editor,
                    element,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if way.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, element, args, flood_fill_cache, road_mask);
            } else if way.tags.contains_key("leisure") {
                leisure::generate_leisure(editor, way, args, flood_fill_cache, building_footprints);
            } else if way.tags.contains_key("barrier") {
                barriers::generate_barriers(editor, element, bridge_surface);
            } else if let Some(val) = way.tags.get("waterway") {
                if val == "dock" {
                    // docks count as water areas
                    water_areas::generate_water_area_from_way(
                        editor,
                        way,
                        xzbbox,
                        big_water_field,
                        road_mask,
                    );
                } else {
                    waterways::generate_waterways(editor, way);
                }
            } else if way.tags.contains_key("railway") {
                railways::generate_railways(
                    editor,
                    way,
                    subway_points,
                    rail_bridge_internal_endpoints,
                    bridge_outlines,
                );
            } else if way.tags.contains_key("roller_coaster") {
                railways::generate_roller_coaster(editor, way);
            } else if way.tags.contains_key("aeroway") || way.tags.contains_key("area:aeroway") {
                highways::generate_aeroway(editor, way, args);
            } else if way.tags.get("service").map(String::as_str) == Some("siding") {
                highways::generate_siding(editor, way, bridge_surface);
            } else if way.tags.get("tomb").map(String::as_str) == Some("pyramid") {
                historic::generate_pyramid(editor, way, args, flood_fill_cache);
            } else if way.tags.contains_key("man_made") {
                man_made::generate_man_made(editor, element, args);
            } else if way.tags.contains_key("power") {
                power::generate_power(editor, element);
            } else if way.tags.contains_key("place") {
                landuse::generate_place(editor, way, args, flood_fill_cache);
            }
        }
        ProcessedElement::Node(node) => {
            if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                doors::generate_doors(editor, node);
            } else if node.tags.get("natural").map(String::as_str) == Some("tree") {
                natural::generate_natural(
                    editor,
                    element,
                    args,
                    flood_fill_cache,
                    building_footprints,
                );
            } else if node.tags.contains_key("amenity") {
                amenities::generate_amenities(editor, element, args, flood_fill_cache, road_mask);
            } else if node.tags.contains_key("barrier") {
                barriers::generate_barrier_nodes(editor, node, bridge_surface);
            } else if node.tags.contains_key("highway") {
                highways::generate_highways(
                    editor,
                    element,
                    args,
                    highway_connectivity,
                    flood_fill_cache,
                    road_mask,
                    bridge_structures,
                    bridge_surface,
                );
            } else if node.tags.contains_key("tourism") {
                tourisms::generate_tourisms(editor, node);
            } else if node.tags.contains_key("man_made") {
                man_made::generate_man_made_nodes(editor, node, args);
            } else if node.tags.contains_key("power") {
                power::generate_power_nodes(editor, node);
            } else if node.tags.contains_key("historic") {
                historic::generate_historic(editor, node);
            } else if node.tags.contains_key("emergency") {
                emergency::generate_emergency(editor, node);
            } else if node.tags.contains_key("advertising") {
                advertising::generate_advertising(editor, node);
            }
        }
        ProcessedElement::Relation(rel) => {
            let is_building_relation = rel.tags.contains_key("building")
                || rel.tags.contains_key("building:part")
                || rel.tags.get("type").map(|t| t.as_str()) == Some("building");
            if is_building_relation {
                buildings::generate_building_from_relation(
                    editor,
                    rel,
                    args,
                    flood_fill_cache,
                    xzbbox,
                    building_passages,
                );
            } else if rel.tags.contains_key("water")
                || rel
                    .tags
                    .get("natural")
                    .map(|val| val == "water" || val == "bay")
                    .unwrap_or(false)
            {
                water_areas::generate_water_areas_from_relation(
                    editor,
                    rel,
                    xzbbox,
                    big_water_field,
                    road_mask,
                );
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
            } else if rel.tags.get("leisure").map(String::as_str) == Some("park") {
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

/// Whether to stream regions to disk (lower peak RAM) for `num_regions` regions. Auto-enabled
/// when the estimated resident world would crowd available RAM; trades some time for RAM, output
/// unchanged (3D models + subways preserved). `ARNIS_STREAM_TO_DISK=1/0` overrides; constants tunable.
fn should_stream_to_disk(num_regions: usize) -> bool {
    match std::env::var("ARNIS_STREAM_TO_DISK").ok().as_deref() {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    // Calibrated on a dense full-feature run (terrain + land cover + Overture + 3D): ~26 MB/region.
    const BASE_MB: u64 = 500;
    const PER_REGION_MB: u64 = 26;
    let est_peak_mb = BASE_MB + PER_REGION_MB * num_regions as u64;

    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let available_mb = sys.available_memory() / (1024 * 1024);
    // Stream once the estimate would use >55% of available RAM (unknown memory -> fast path).
    available_mb > 0 && est_peak_mb * 100 > available_mb * 55
}

/// Generate world with explicit format options (used by GUI for Bedrock support)
pub fn generate_world_with_options(
    elements: Vec<ProcessedElement>,
    xzbbox: XZBBox,
    llbbox: LLBBox,
    ground: Ground,
    args: &Args,
    options: GenerationOptions,
    outline_suppression: OutlineSuppression,
) -> Result<PathBuf, String> {
    let output_path = options.path.clone();
    let world_format = options.format;
    let generation_start = args.benchmark.then(std::time::Instant::now);

    // Create editor with appropriate format
    let mut editor: WorldEditor = if options.format == WorldFormat::LuantiWorld {
        WorldEditor::new_luanti(
            options.path,
            &xzbbox,
            llbbox,
            options
                .luanti_game
                .unwrap_or(crate::luanti_block_map::LuantiGame::Mineclonia),
            options.level_name.clone(),
            options.spawn_point,
            options.ground_level,
        )
    } else {
        WorldEditor::new_with_format_and_name(
            options.path,
            &xzbbox,
            llbbox,
            options.format,
            options.level_name.clone(),
            options.spawn_point,
            args.disable_height_limit,
        )
    };
    editor.set_bake_lighting(args.bake_lighting);
    editor.set_projection_info(&args.projection.to_string(), args.scale);
    let ground = Arc::new(ground);
    let mut bench = crate::bench::Bench::new(args.benchmark);

    // Per-cell water depth field from the LC_WATER mask; empty without land cover.
    let big_water_field = crate::water_depth::compute_big_water_field(&ground, &xzbbox);

    println!("{} Processing data...", "[4/7]".bold());

    // Build highway connectivity map once before processing
    let highway_connectivity = highways::build_highway_connectivity_map(&elements);

    // Collect subway centerline points for post-ground-fill air carving (phase 2).
    let mut subway_points: Vec<(i32, i32)> = Vec::new();

    // Set ground reference in the editor to enable elevation-aware block placement
    editor.set_ground(Arc::clone(&ground));

    println!("{} Generating area...", "[5/7]".bold());
    emit_gui_progress_update(20.0, "Generating area...");

    // Pre-compute all flood fills in parallel for better CPU utilization
    let mut flood_fill_cache = FloodFillCache::precompute(&elements, args.timeout.as_ref());

    // Collect building footprints to prevent trees from spawning inside buildings
    // Uses a memory-efficient bitmap (~1 bit per coordinate) instead of a HashSet (~24 bytes per coordinate)
    let building_footprints = flood_fill_cache.collect_building_footprints(&elements, &xzbbox);

    // Collect coordinates covered by tunnel=building_passage highways so that
    // building generation can cut ground-level openings through walls and floors.
    let building_passages =
        highways::collect_building_passage_coords(&elements, &xzbbox, args.scale);

    // Pre-build a bitmap of every (x, z) block coordinate covered by a rendered
    // road or path surface. Uses the same Bresenham + block_range geometry as
    // generate_highways_internal, so the bitmap is a 1:1 match of what gets placed.
    // Amenity processors use this for O(1) nearest-road-block lookups.
    let road_mask = highways::collect_road_surface_coords(&elements, &xzbbox, args.scale);

    let bridge_outlines =
        crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&elements);
    let bridge_structures =
        bridges::BridgeStructureMap::build(&elements, &editor, &bridge_outlines);
    let bridge_surface =
        bridges::BridgeSurfaceMap::build(&elements, &bridge_structures, args.scale);

    let rail_bridge_internal_endpoints =
        railways::collect_rail_bridge_internal_endpoints(&elements);

    // 3D model pipeline pre-scan: elements rendered as 3D models instead of
    // voxels are recorded here and skipped by the element loop below.
    let models_3d_pipeline = args
        .use_3d
        .then(|| crate::models_3d::Models3dPipeline::prescan(&elements, args));
    let empty_suppressed: HashSet<(&'static str, u64)> = HashSet::new();
    let models_3d_suppressed: &HashSet<(&'static str, u64)> = models_3d_pipeline
        .as_ref()
        .map(|p| p.suppressed())
        .unwrap_or(&empty_suppressed);

    bench.mark("precompute");

    // Stream-to-disk eviction state (populated in the parallel branch below).
    let mut eviction_active = false;
    let hash_check = std::env::var_os("ARNIS_BLOCK_HASH").is_some();
    let mut hash_acc: u64 = 0;
    let mut real_regions: HashSet<(i32, i32)> = HashSet::new();
    let mut evicted_regions: HashSet<(i32, i32)> = HashSet::new();
    // Background writer for eviction; None unless eviction is active.
    let mut flush_worker: Option<FlushWorker> = None;

    // Decide between sequential and parallel processing based on world size.
    // Tile subdivision is aligned to 512-block Minecraft region boundaries.
    let tiles = tile::create_tiles(&xzbbox, tile::DEFAULT_TILE_SIZE);

    // Tile editors are created as JavaAnvil (WorldEditor::new), so their
    // format-dependent block-entity schema (banners) only matches Java output.
    // Restrict the parallel tile path to Java; Bedrock/Luanti large worlds use
    // the sequential path (correct, just not tile-parallel).
    let use_parallel_tiles = tiles.len() >= 3 && matches!(world_format, WorldFormat::JavaAnvil);

    if use_parallel_tiles {
        // Large area: process tiles in parallel using rayon.
        // Each tile gets its own WorldEditor with an expanded bounding box (64-block
        // halo) so that elements whose centroid falls inside the tile can render blocks
        // that extend slightly beyond the strict tile boundary (e.g., wide buildings).
        // After each batch finishes, their WorldToModify results are merged back into the
        // main editor using authoritative bounds (strict tile area overwrites; halo
        // writes only if the target position is still AIR).
        //
        // Tiles are processed in batches (one tile per rayon thread) to cap peak memory.
        // Without batching, all tile WorldToModify structs would be in memory at once,
        // which can exceed RAM for large areas and cause disk thrashing.
        let tile_batch_size = rayon::current_num_threads().max(1);
        println!(
            "  Processing {} tiles across {tile_batch_size} threads...",
            tiles.len()
        );

        let tile_assignments = tile::assign_elements_to_tiles(&elements, &tiles, args.scale);

        // Stream-to-disk: flush+evict each region once its owner + 8 neighbour tiles merge,
        // auto-enabled when the resident world would crowd available RAM. Java only; 3D models
        // are kept via region deferral.
        eviction_active =
            matches!(world_format, WorldFormat::JavaAnvil) && should_stream_to_disk(tiles.len());

        // Regions any 3D placement may write to: kept resident (not evicted in-loop)
        // so the post-merge placement pass lands in RAM, then flushed at finalize.
        let model_regions: HashSet<(i32, i32)> = if eviction_active {
            models_3d_pipeline
                .as_ref()
                .map(|p| p.deferred_region_keys(args.scale))
                .unwrap_or_default()
        } else {
            HashSet::new()
        };

        if eviction_active {
            flush_worker = Some(FlushWorker::spawn(editor.region_write_ctx(), 3));
        }

        let mut indexed_tiles: Vec<(usize, &tile::TileBounds)> = tiles.iter().enumerate().collect();
        // ARNIS_NO_BAND forces LPT order even under eviction (debug: isolates ordering effects).
        let band = eviction_active && std::env::var_os("ARNIS_NO_BAND").is_none();
        if band {
            // Row-major bands (LPT within a row) so the seal frontier sweeps top-to-bottom.
            indexed_tiles.sort_by(|a, b| {
                let za = a.1.min_z >> 9;
                let zb = b.1.min_z >> 9;
                za.cmp(&zb).then_with(|| {
                    tile_assignments[b.0]
                        .len()
                        .cmp(&tile_assignments[a.0].len())
                })
            });
        } else {
            // LPT scheduling: dense tiles first so a straggler doesn't block the pipeline.
            indexed_tiles.sort_by(|a, b| {
                tile_assignments[b.0]
                    .len()
                    .cmp(&tile_assignments[a.0].len())
            });
        }

        let region_of_tile: Vec<(i32, i32)> =
            tiles.iter().map(|t| (t.min_x >> 9, t.min_z >> 9)).collect();
        real_regions = region_of_tile.iter().copied().collect();
        // remaining[R] = 1 (owner) + count of R's in-grid region neighbours; R is
        // flushable when this reaches 0 (owner + all neighbour tiles merged).
        let mut remaining: HashMap<(i32, i32), u32> = HashMap::new();
        if eviction_active {
            for &r in &real_regions {
                let mut c = 1u32;
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if (dx, dz) != (0, 0) && real_regions.contains(&(r.0 + dx, r.1 + dz)) {
                            c += 1;
                        }
                    }
                }
                remaining.insert(r, c);
            }
        }

        let mut place_dur = std::time::Duration::ZERO;
        let mut merge_dur = std::time::Duration::ZERO;
        let total_tiles = indexed_tiles.len().max(1);
        let mut tiles_merged = 0usize;
        let mut last_emitted_pct = 20.0_f64;
        for batch in indexed_tiles.chunks(tile_batch_size) {
            // Phase 1: process this batch of tiles in parallel
            let place_start = std::time::Instant::now();
            let batch_results: Vec<_> = batch
                .par_iter()
                .map(|&(tile_idx, tile_bounds)| {
                    // max_* are exclusive; rect_from_min_max treats max as inclusive,
                    // so subtract 1. Clamp to the world bbox so edge-tile halos don't
                    // extend past world bounds.
                    let tile_xzbbox = XZBBox::rect_from_min_max(
                        (tile_bounds.min_x - tile::TILE_EDITOR_HALO).max(xzbbox.min_x()),
                        (tile_bounds.min_z - tile::TILE_EDITOR_HALO).max(xzbbox.min_z()),
                        (tile_bounds.max_x - 1 + tile::TILE_EDITOR_HALO).min(xzbbox.max_x()),
                        (tile_bounds.max_z - 1 + tile::TILE_EDITOR_HALO).min(xzbbox.max_z()),
                    )
                    .expect("Failed to create tile XZBBox");

                    let mut tile_editor = WorldEditor::new(PathBuf::new(), &tile_xzbbox, llbbox);
                    tile_editor.set_ground(Arc::clone(&ground));
                    tile_editor.set_ground_origin(xzbbox.min_x(), xzbbox.min_z());

                    let mut tile_subway_points: Vec<(i32, i32)> = Vec::new();

                    for &elem_idx in &tile_assignments[tile_idx] {
                        let element = &elements[elem_idx];
                        let suppression_key = (element.kind(), element.id());
                        if models_3d_suppressed.contains(&suppression_key)
                            || outline_suppression.contains(&suppression_key)
                        {
                            continue;
                        }
                        process_element(
                            &mut tile_editor,
                            element,
                            args,
                            &highway_connectivity,
                            &flood_fill_cache,
                            &building_footprints,
                            &building_passages,
                            &road_mask,
                            // World bbox (not tile) for relation/area ring clipping: clipping to
                            // the tile can drop a relation whose ring fails to close. The tile
                            // editor still bounds the actual writes.
                            &xzbbox,
                            &big_water_field,
                            &bridge_structures,
                            &bridge_surface,
                            &bridge_outlines,
                            &rail_bridge_internal_endpoints,
                            &mut tile_subway_points,
                        );
                    }

                    // Per-tile ground + ore + ESA-water over strict bounds (parallel);
                    // neighbour reads use the editor halo from intersection assignment.
                    let g_min_x = tile_bounds.min_x.max(xzbbox.min_x());
                    let g_max_x = (tile_bounds.max_x - 1).min(xzbbox.max_x());
                    let g_min_z = tile_bounds.min_z.max(xzbbox.min_z());
                    let g_max_z = (tile_bounds.max_z - 1).min(xzbbox.max_z());
                    ground_generation::generate_ground_region(
                        &mut tile_editor,
                        ground.as_ref(),
                        args,
                        &xzbbox,
                        &building_footprints,
                        g_min_x,
                        g_max_x,
                        g_min_z,
                        g_max_z,
                        false,
                    );
                    if args.fillground {
                        crate::ore_generation::generate_ores_region(
                            &mut tile_editor,
                            g_min_x,
                            g_max_x,
                            g_min_z,
                            g_max_z,
                            false,
                        );
                    }
                    crate::water_depth::carve_lc_water_region(
                        &mut tile_editor,
                        ground.as_ref(),
                        &xzbbox,
                        &big_water_field,
                        &road_mask,
                        g_min_x,
                        g_max_x,
                        g_min_z,
                        g_max_z,
                    );

                    // Under eviction the post-merge subway carve can't run (regions get freed),
                    // so carve in-tile now, after ground/fill so the interior isn't refilled.
                    if eviction_active {
                        railways::carve_subway_interior(&mut tile_editor, &tile_subway_points);
                    }

                    let tile_road_overrides = tile_editor.take_road_surface_overrides();
                    (
                        tile_idx,
                        tile_editor.into_world(),
                        tile_subway_points,
                        tile_road_overrides,
                    )
                })
                .collect();
            place_dur += place_start.elapsed();

            let merge_start = std::time::Instant::now();
            // Phase 2: merge this batch's results into the main editor (sequential).
            // batch_results is dropped after this loop, freeing memory before next batch.
            for (tile_idx, tile_world, tile_subway_pts, tile_road_overrides) in batch_results {
                editor.merge_world(
                    tile_world,
                    tiles[tile_idx].min_x,
                    tiles[tile_idx].min_z,
                    tiles[tile_idx].max_x - 1,
                    tiles[tile_idx].max_z - 1,
                );
                // Carry road-surface overrides to the main editor so the post-merge 3D-model
                // pass stays road-aware. Under eviction keep only the deferred 3D regions'
                // overrides (the rest are evicted; this caps the extra resident RAM).
                if eviction_active {
                    editor.merge_road_surface_overrides_in_regions(
                        tile_road_overrides,
                        &model_regions,
                    );
                } else {
                    editor.merge_road_surface_overrides(tile_road_overrides);
                }

                if eviction_active {
                    // This tile contributes to its own region and its 8 neighbours;
                    // flush each non-deferred region whose contributors are all merged.
                    // (Subways are carved in-tile above, so they don't defer regions.)
                    let rt = region_of_tile[tile_idx];
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            let d = (rt.0 + dx, rt.1 + dz);
                            if let Some(c) = remaining.get_mut(&d) {
                                *c -= 1;
                                if *c == 0
                                    && !evicted_regions.contains(&d)
                                    && !model_regions.contains(&d)
                                {
                                    if hash_check {
                                        hash_acc = hash_acc
                                            .wrapping_add(editor.region_content_hash(d.0, d.1));
                                    }
                                    if let Some(w) = flush_worker.as_ref() {
                                        editor.flush_region_via(w, d.0, d.1)?;
                                    }
                                    evicted_regions.insert(d);
                                }
                            }
                        }
                    }
                }

                subway_points.extend(tile_subway_pts);

                // Step 20%->70% per merged tile, throttled to whole-percent steps.
                tiles_merged += 1;
                let pct = 20.0 + (tiles_merged as f64 / total_tiles as f64) * 50.0;
                if pct - last_emitted_pct >= 1.0 {
                    emit_gui_progress_update_ex(pct, "Generating area...", eviction_active);
                    last_emitted_pct = pct;
                }
            }
            merge_dur += merge_start.elapsed();
        }
        bench.report("element_placement", place_dur);
        bench.report("tile_merge", merge_dur);
        bench.reset();

        if eviction_active && args.benchmark {
            eprintln!(
                "[BENCHMARK] evicted_in_loop={} model_deferred={} real_regions={}",
                evicted_regions.len(),
                model_regions.len(),
                real_regions.len()
            );
        }

        emit_gui_progress_update_ex(70.0, "", eviction_active);
    } else {
        // Small area: sequential processing along the original code path.
        let elements_count: usize = elements.len();
        let process_pb: ProgressBar = ProgressBar::new(elements_count as u64);
        process_pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
            .unwrap()
            .progress_chars("█▓░"));

        let progress_increment_prcs: f64 = 50.0 / elements_count as f64;
        let mut current_progress_prcs: f64 = 20.0;
        let mut last_emitted_progress: f64 = current_progress_prcs;
        let desired_updates: u64 = 500;
        let pb_batch_size: u64 = (elements_count as u64 / desired_updates).max(1);
        let mut element_counter: u64 = 0;

        for element in elements.into_iter() {
            element_counter += 1;
            let suppression_key = (element.kind(), element.id());
            if models_3d_suppressed.contains(&suppression_key)
                || outline_suppression.contains(&suppression_key)
            {
                continue;
            }
            if element_counter.is_multiple_of(pb_batch_size) {
                process_pb.inc(pb_batch_size);
            }
            current_progress_prcs += progress_increment_prcs;
            if (current_progress_prcs - last_emitted_progress).abs() > 0.25 {
                emit_gui_progress_update(current_progress_prcs, "");
                last_emitted_progress = current_progress_prcs;
            }

            if args.debug {
                process_pb.set_message(format!(
                    "(Element ID: {} / Type: {})",
                    element.id(),
                    element.kind()
                ));
            } else {
                // Clear on every non-debug iteration so any transient warning
                // message set by downstream element processing (missing nodes,
                // etc.) doesn't stick for the rest of the run.
                process_pb.set_message("");
            }

            process_element(
                &mut editor,
                &element,
                args,
                &highway_connectivity,
                &flood_fill_cache,
                &building_footprints,
                &building_passages,
                &road_mask,
                &xzbbox,
                &big_water_field,
                &bridge_structures,
                &bridge_surface,
                &bridge_outlines,
                &rail_bridge_internal_endpoints,
                &mut subway_points,
            );

            // Release flood fill cache entries for memory optimization.
            // (Skipped in the parallel path where the cache is shared immutably.)
            match &element {
                ProcessedElement::Way(way) => {
                    flood_fill_cache.remove_way(way.id);
                }
                ProcessedElement::Relation(rel) => {
                    let way_ids: Vec<u64> = rel.members.iter().map(|m| m.way.id).collect();
                    flood_fill_cache.remove_relation_ways(&way_ids);
                }
                _ => {}
            }
            // Element is dropped here, freeing its memory immediately.
        }

        process_pb.inc(element_counter % pb_batch_size);
        process_pb.finish();
        bench.mark("elements_sequential");
    }

    // Keep road_mask alive for the LC_WATER carve below.
    drop(highway_connectivity);
    drop(flood_fill_cache);

    // True when ground (and the ore/water post-passes) run on the merged editor:
    // the small-area sequential path, or the whole-bbox-ground override. The
    // parallel per-tile path already did ground + ore + water inside the closure.
    let ground_on_merged = !use_parallel_tiles;

    if ground_on_merged {
        ground_generation::generate_ground_layer(
            &mut editor,
            ground.as_ref(),
            args,
            &xzbbox,
            &building_footprints,
        )?;
    }
    bench.mark("ground_gen");

    if ground_on_merged {
        if args.fillground {
            crate::ore_generation::generate_ores(&mut editor, &xzbbox);
        }
        // Carve depth into ESA water cells (water_areas.rs only covers OSM polygons).
        crate::water_depth::carve_lc_water_pass(
            &mut editor,
            ground.as_ref(),
            &xzbbox,
            &big_water_field,
            &road_mask,
        );
    }

    drop(road_mask);

    // Carve subway tunnel interiors now that underground is filled with stone.
    // Under eviction this already ran in-tile (regions get freed before here).
    if !eviction_active && !subway_points.is_empty() {
        railways::carve_subway_interior(&mut editor, &subway_points);
    }

    // Run after ground generation so anchor Y reflects the final terrain.
    if let Some(p) = models_3d_pipeline.as_ref() {
        p.place(&mut editor, args);
    }
    bench.mark("post_passes");

    if eviction_active {
        // Flush deferred (subway-touched) regions now the global carve has run on them.
        let mut leftover: Vec<(i32, i32)> =
            real_regions.difference(&evicted_regions).copied().collect();
        leftover.sort_unstable();
        for (rx, rz) in leftover {
            if hash_check {
                hash_acc = hash_acc.wrapping_add(editor.region_content_hash(rx, rz));
            }
            if let Some(w) = flush_worker.as_ref() {
                editor.flush_region_via(w, rx, rz)?;
            }
            evicted_regions.insert((rx, rz));
        }
        // Hash remaining (out-of-bbox halo) regions so hash_acc == the whole-world hash.
        if hash_check {
            for (rx, rz) in editor.resident_region_keys() {
                hash_acc = hash_acc.wrapping_add(editor.region_content_hash(rx, rz));
            }
        }
        // Wait for all background writes to land (and surface any I/O error) before save.
        if let Some(w) = flush_worker.take() {
            w.finish()?;
        }
    }

    if hash_check {
        let h = if eviction_active {
            hash_acc
        } else {
            editor.content_hash()
        };
        eprintln!("[BENCHMARK] block_hash={:016x}", h);
    }

    // Save world
    if let Err(e) = editor.save() {
        return Err(e.to_string());
    }
    bench.mark("save");

    if let Some(start) = generation_start {
        let gen_ms = start.elapsed().as_millis();
        eprintln!("[BENCHMARK] generation_time_ms={gen_ms}");
    }

    emit_gui_progress_update(99.0, "Finalizing world...");

    // Update player spawn Y coordinate based on terrain height after generation
    #[cfg(feature = "gui")]
    if world_format == WorldFormat::JavaAnvil {
        use crate::gui::update_player_spawn_y_after_generation;
        // Reconstruct bbox string to match the format that GUI originally provided.
        // This ensures LLBBox::from_str() can parse it correctly.
        let bbox_string = format!(
            "{},{},{},{}",
            args.bbox.min().lat(),
            args.bbox.min().lng(),
            args.bbox.max().lat(),
            args.bbox.max().lng()
        );

        // Always update spawn Y since we now always set a spawn point (user-selected or default)
        if let Some(ref world_path) = args.path {
            if let Err(e) = update_player_spawn_y_after_generation(
                world_path,
                bbox_string,
                args.scale,
                ground.as_ref(),
            ) {
                let warning_msg = format!("Failed to update spawn point Y coordinate: {}", e);
                eprintln!("Warning: {}", warning_msg);
                #[cfg(feature = "gui")]
                send_log(LogLevel::Warning, &warning_msg);
            }
        }
    }

    // For Bedrock format, emit event to open the mcworld file
    if world_format == WorldFormat::BedrockMcWorld {
        if let Some(path_str) = output_path.to_str() {
            emit_show_in_folder(path_str);
        }
    }

    // For Java worlds saved to the Desktop (GUI falls back there when .minecraft/saves
    // is missing), open the folder in the file explorer so the user can find the world.
    if world_format == WorldFormat::JavaAnvil {
        if let Some(desktop) = dirs::desktop_dir() {
            if output_path.starts_with(&desktop) {
                if let Some(path_str) = output_path.to_str() {
                    emit_show_in_folder(path_str);
                }
            }
        }
    }

    Ok(output_path)
}
