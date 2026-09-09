//! Building facades from Mapillary street-level photographs.
//!
//! [`pipeline`] turns a bounding box and a token into the export
//! [`facades::install`] loads: per wall a one metre class grid with colours and
//! an eight pixel per metre texture, per building a colour and a band list.
//! [`FacadeJob`] is how generation asks for it, and the reason it is a job
//! rather than a call is that the download is the dominant cost and there is a
//! minute of unrelated fetching and parsing to hide it behind.
//!
//! What remains here of the older colour-only pass is the coverage probe:
//! [`sample_area`] and [`report`] answer "does this area have imagery at all"
//! from the metadata and a sample of panoramas, without generating a world.
//! `--mapillary-probe` is its only caller. Generation itself no longer uses it,
//! because the pipeline supersedes it; see the comment on [`sample_area`].

pub mod api;
pub mod atlas;
pub mod credits;
pub mod displays;
pub mod facade;
pub mod facades;
pub mod project;

// The orthofacade port, one module per module of `tools/facade_lab/`, each
// file's header naming the Python module it corresponds to. The stages run in
// the order fetch, geometry with pose and sfm, visibility, register, plane,
// rectify, refine, fuse, openings, bands, confidence, and `pipeline` drives
// them; `types` is what they all share.
pub mod bands;
/// The on-disk cache the fetch stage fills and the pipeline reads back.
pub mod cache;
pub mod confidence;
pub mod fetch;
pub mod fuse;
pub mod geometry;
/// Loading the golden fixtures; each stage's golden test lives in its own file.
#[cfg(test)]
pub mod golden;
/// Morphology, connected components and the OkLab pair the stages share.
pub mod imgops;
pub mod openings;
pub mod pipeline;
pub mod plane;
pub mod pose;
pub mod rectify;
pub mod refine;
pub mod register;
pub mod sfm;
pub mod types;
pub mod visibility;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use colored::Colorize;

use crate::args::Args;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};
use facade::BuildingSample;
use project::CameraPose;

/// Mean earth radius, matching `coordinate_system::transformation`.
const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Baseline for the bearing probe below. Long enough that the parser's integer
/// node rounding costs well under a degree of heading.
const BEARING_PROBE_M: f64 = 200.0;

/// Per-request timeout for both metadata and image fetches.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Panoramas downloaded per run. Metadata is cheap; pixels are not, and a dense
/// city centre can list thousands of panoramas inside a single bbox.
const MAX_PANORAMA_DOWNLOADS: usize = 600;

/// Destination point `distance_m` away along `bearing_deg`, on a sphere.
fn offset_point(lat: f64, lon: f64, bearing_deg: f64, distance_m: f64) -> Option<LLPoint> {
    let (lat1, lon1) = (lat.to_radians(), lon.to_radians());
    let theta = bearing_deg.to_radians();
    let delta = distance_m / EARTH_RADIUS_M;

    let lat2 = (lat1.sin() * delta.cos() + lat1.cos() * delta.sin() * theta.cos()).asin();
    let lon2 = lon1
        + (theta.sin() * delta.sin() * lat1.cos()).atan2(delta.cos() - lat1.sin() * lat2.sin());

    LLPoint::new(lat2.to_degrees(), lon2.to_degrees().clamp(-180.0, 180.0)).ok()
}

/// Projects a lat/lon into world XZ exactly the way the OSM parser does,
/// including the map rotation.
fn world_xz(lat: f64, lon: f64, llbbox: LLBBox, args: &Args) -> Option<(i32, i32)> {
    let llpoint = LLPoint::new(lat, lon).ok()?;
    let (transformer, pre_rotation_bbox) = match args.projection {
        crate::projection::ProjectionKind::WebMercator => {
            let origin_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
            let origin_lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;
            let proj =
                crate::projection::WebMercatorProjection::new(origin_lat, origin_lon, args.scale);
            CoordTransformer::with_projection(&llbbox, args.scale, &proj)
        }
        crate::projection::ProjectionKind::Local => {
            CoordTransformer::llbbox_to_xzbbox(&llbbox, args.scale)
        }
    }
    .ok()?;

    let xzpoint = transformer.transform_point(llpoint);
    Some(crate::map_transformation::rotate::rotate_xz_point(
        xzpoint.x,
        xzpoint.z,
        args.rotation,
        &pre_rotation_bbox,
    ))
}

/// Turns a panorama's geographic pose into a world-space camera pose.
///
/// The heading is recovered by projecting a second point along the compass
/// bearing and measuring the result in world space. Deriving it that way means
/// the projection's own distortion and the map rotation are both accounted for
/// without this module having to know anything about either.
fn camera_pose(meta: &api::PanoMeta, llbbox: LLBBox, args: &Args) -> Option<CameraPose> {
    let (x, z) = world_xz(meta.lat, meta.lon, llbbox, args)?;

    let ahead = offset_point(meta.lat, meta.lon, meta.compass_angle, BEARING_PROBE_M)?;
    let (fx, fz) = world_xz(ahead.lat(), ahead.lng(), llbbox, args)?;

    let (dx, dz) = ((fx - x) as f64, (fz - z) as f64);
    if dx == 0.0 && dz == 0.0 {
        return None;
    }

    let world_bearing = project::bearing_deg(dx, dz);
    // Wall heights are measured from the building base, and Arnis has no
    // per-building terrain offset at this stage, so both sit on a shared
    // local zero.
    let (cx, cy, cz) = (x as f64, facade::CAMERA_HEIGHT_M * args.scale, z as f64);

    Some(match meta.rotation {
        Some(rotation) => CameraPose::oriented(
            cx,
            cy,
            cz,
            world_bearing,
            rotation,
            // Derived from the probe rather than `args.rotation` directly, so
            // the projection's own convergence is folded in with the map turn.
            world_bearing - meta.compass_angle,
        ),
        // No SfM orientation: a level camera is all the heading can say.
        None => CameraPose::level(cx, cy, cz, world_bearing),
    })
}

/// A footprint ready to sample: way id, ring in world blocks, height in metres.
type Footprint = (u64, Vec<(f64, f64)>, f64);

/// Every building footprint in the area, keyed the way `BuildingStyle::resolve`
/// keys them.
///
/// Multipolygon relations are walked into their outer members, because that is
/// the `ProcessedWay` the generator resolves a style against.
fn building_footprints(elements: &[ProcessedElement]) -> Vec<Footprint> {
    fn push_way(way: &ProcessedWay, out: &mut Vec<Footprint>) {
        if way.nodes.len() < 4 {
            return;
        }
        let points: Vec<(f64, f64)> = way.nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
        let height = facade::building_height_m(&way.tags);
        out.push((way.id, points, height));
    }

    let mut out = Vec::new();
    for element in elements {
        match element {
            ProcessedElement::Way(way) if way.tags.contains_key("building") => {
                push_way(way, &mut out)
            }
            ProcessedElement::Relation(relation)
                if relation.tags.contains_key("building")
                    || relation.tags.contains_key("building:part") =>
            {
                for member in &relation.members {
                    if matches!(member.role, crate::osm_parser::ProcessedMemberRole::Outer) {
                        push_way(&member.way, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// What a run found, for the probe report and for the caller's summary line.
pub struct SampleReport {
    pub buildings_total: usize,
    pub panoramas_listed: usize,
    pub panoramas_used: usize,
    pub samples: Vec<BuildingSample>,
}

/// Runs the coverage probe over an area: one dominant wall colour per building,
/// from panoramas that see a wall head-on.
///
/// This is what generation used to install, and it no longer does. The pipeline
/// answers the same question better and from the same photographs, so keeping
/// this beside it would have meant fetching the imagery twice for the weaker
/// answer, and the weaker answer would have won: `facade_shell` read
/// this table before the pipeline's own building colour. Where the two do not
/// overlap it is not a fallback either, because the pipeline declines a wall
/// exactly when no candidate view survived the distance, incidence, occlusion
/// and vegetation gates, which is when a single ray lands on a tree or a van.
///
/// What it is still good for is the question `--mapillary-probe` asks, which is
/// whether an area has usable imagery at all, before anyone waits on a
/// generation to find out.
///
/// Returns an empty report when the area has no panoramas; partial coverage is
/// the normal outcome and is reported rather than treated as failure.
pub fn sample_area(
    elements: &[ProcessedElement],
    args: &Args,
    llbbox: LLBBox,
    token: &str,
    keep_grids: bool,
) -> Result<SampleReport, String> {
    let buildings = building_footprints(elements);
    if buildings.is_empty() {
        return Err("no building footprints in this area".to_string());
    }

    emit_gui_progress_update(MESSAGE_ONLY, "Facades: searching coverage...");
    let metas = api::fetch_panoramas(&llbbox, token, HTTP_TIMEOUT)?;
    let listed = metas.len();
    if metas.is_empty() {
        return Ok(SampleReport {
            buildings_total: buildings.len(),
            panoramas_listed: 0,
            panoramas_used: 0,
            samples: Vec::new(),
        });
    }

    // Best-quality panoramas first, so the download cap trims the worst.
    let mut metas = metas;
    metas.sort_by(|a, b| {
        b.quality_score
            .total_cmp(&a.quality_score)
            .then_with(|| a.id.cmp(&b.id))
    });
    metas.truncate(MAX_PANORAMA_DOWNLOADS);

    emit_gui_progress_update(MESSAGE_ONLY, "Facades: downloading panoramas...");
    let panoramas = api::download_panoramas(&metas, HTTP_TIMEOUT)?;
    if panoramas.is_empty() {
        return Err("every panorama download failed".to_string());
    }

    let cameras: Vec<(CameraPose, &api::Panorama)> = panoramas
        .iter()
        .filter_map(|pano| Some((camera_pose(&pano.meta, llbbox, args)?, pano)))
        .collect();

    emit_gui_progress_update(MESSAGE_ONLY, "Facades: sampling walls...");
    let samples: Vec<BuildingSample> = buildings
        .iter()
        .filter_map(|(way_id, points, height)| {
            facade::sample_building(*way_id, points, *height, &cameras, args.scale, keep_grids)
        })
        .collect();

    Ok(SampleReport {
        buildings_total: buildings.len(),
        panoramas_listed: listed,
        panoramas_used: panoramas.len(),
        samples,
    })
}

/// Prints the probe summary and, when a directory is given, writes one PNG per
/// sampled building showing the reconstructed facade grids.
///
/// The grids are the real check on this pass: if the projection is aimed
/// correctly they read as a recognisable, very low resolution building. If the
/// heading convention or the winding were wrong they would be noise.
pub fn report(report: &SampleReport, debug_dir: Option<&std::path::Path>) -> Result<(), String> {
    let covered = report.samples.len();
    let share = if report.buildings_total == 0 {
        0.0
    } else {
        100.0 * covered as f64 / report.buildings_total as f64
    };

    println!();
    println!("{}", "Mapillary facade probe".bold());
    println!(
        "  Panoramas: {} listed, {} downloaded",
        report.panoramas_listed, report.panoramas_used
    );
    println!(
        "  Buildings: {covered} of {} sampled ({share:.1}%)",
        report.buildings_total
    );

    if covered == 0 {
        println!(
            "  {}",
            "No building had a wall a panorama could see head-on.".yellow()
        );
        return Ok(());
    }

    let mut by_confidence: Vec<&BuildingSample> = report.samples.iter().collect();
    by_confidence.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.way_id.cmp(&b.way_id))
    });

    println!();
    println!(
        "  {:<12} {:>9} {:>6} {:>6} {:>6} {:>16}  block",
        "way", "colour", "conf", "cells", "walls", "world x/z"
    );
    for sample in by_confidence.iter().take(25) {
        let (r, g, b) = sample.color;
        // Same deterministic seeding the generator uses, so the block shown
        // here is the block the world will get.
        let block = crate::block_palette::wall_block_for_color(
            sample.color,
            &mut crate::deterministic_rng::element_rng(sample.way_id),
        );
        println!(
            "  {:<12} #{:02x}{:02x}{:02x}  {:>5.0}% {:>6} {:>6} {:>16}  {}",
            sample.way_id,
            r,
            g,
            b,
            sample.confidence * 100.0,
            sample.accepted_cells,
            sample.walls_sampled,
            format!("{} / {}", sample.world_xz.0, sample.world_xz.1),
            block.name(),
        );
    }
    if covered > 25 {
        println!("  ... and {} more", covered - 25);
    }

    let rejects = report
        .samples
        .iter()
        .fold(facade::RejectStats::default(), |mut acc, s| {
            acc.too_dark += s.rejects.too_dark;
            acc.too_bright += s.rejects.too_bright;
            acc.vegetation += s.rejects.vegetation;
            acc.sky += s.rejects.sky;
            acc.no_view += s.rejects.no_view;
            acc
        });
    println!();
    println!(
        "  Rejected samples: {} total ({} vegetation, {} sky, {} dark, {} bright)",
        rejects.total(),
        rejects.vegetation,
        rejects.sky,
        rejects.too_dark,
        rejects.too_bright
    );

    if let Some(dir) = debug_dir {
        let written = write_debug_grids(report, dir)?;
        println!();
        println!("  Wrote {written} facade grids to {}", dir.display());
    }

    Ok(())
}

/// Pixels per block in the sheet. A whole multiple of `PREVIEW_PX_PER_BLOCK`
/// so the photograph and the block grid land at exactly the same width and can
/// be read against each other column by column.
const DEBUG_CELL_PX: u32 = 16;

/// Copies `src` into `dst` at `(ox, oy)`, scaling each source pixel to a
/// `scale`-by-`scale` block.
fn blit_scaled(dst: &mut image::RgbImage, src: &image::RgbImage, ox: u32, oy: u32, scale: u32) {
    for y in 0..src.height() {
        for x in 0..src.width() {
            let px = *src.get_pixel(x, y);
            for dy in 0..scale {
                for dx in 0..scale {
                    let (tx, ty) = (ox + x * scale + dx, oy + y * scale + dy);
                    if tx < dst.width() && ty < dst.height() {
                        dst.put_pixel(tx, ty, px);
                    }
                }
            }
        }
    }
}

/// Writes one comparison sheet per sampled building.
///
/// Top: the wall re-projected at eight samples per block, unfiltered - what the
/// camera actually saw, ortho-rectified onto the wall plane. Middle: the same
/// wall at one sample per block after filtering and the cross-view median -
/// what Arnis works from. Bottom: the single colour that came out, which is
/// what picks the block. Read together they show the whole reduction, and the
/// top panel is the one that proves the projection lands on the building.
fn write_debug_grids(report: &SampleReport, dir: &std::path::Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    const GAP: u32 = 8;
    const BAND_MARK_PX: u32 = 3;
    let background = image::Rgb([24, 24, 28]);

    // Which panorama drew each preview, so a sheet can be checked against its
    // source image and pose.
    let mut index = String::from(
        "way_id	pano_id	world_x	world_z	colour	confidence
",
    );

    let mut written = 0;
    for sample in &report.samples {
        if let Some((_, _, pano_id)) = &sample.preview {
            index.push_str(&format!(
                "{}	{}	{}	{}	#{:02x}{:02x}{:02x}	{:.2}
",
                sample.way_id,
                pano_id,
                sample.world_xz.0,
                sample.world_xz.1,
                sample.color.0,
                sample.color.1,
                sample.color.2,
                sample.confidence
            ));
        }
        let grid_w = sample.grids.iter().map(|g| g.cols).max().unwrap_or(0) as u32;
        let grid_h: u32 = sample.grids.iter().map(|g| g.rows as u32 + 1).sum();
        if grid_w == 0 || grid_h == 0 {
            continue;
        }

        // The preview is rendered at 8 samples per block, so showing it at
        // 12/8 px per sample puts it at the same physical width as the grid.
        let preview_scale = (DEBUG_CELL_PX / facade::PREVIEW_PX_PER_BLOCK).max(1);
        let preview_size = sample
            .preview
            .as_ref()
            .map(|(img, _, _)| (img.width() * preview_scale, img.height() * preview_scale));

        let body_w = grid_w * DEBUG_CELL_PX;
        let width = body_w.max(preview_size.map_or(0, |(w, _)| w)) + BAND_MARK_PX;
        let preview_h = preview_size.map_or(0, |(_, h)| h + GAP);
        let height = preview_h + grid_h * DEBUG_CELL_PX + DEBUG_CELL_PX;

        let mut img = image::RgbImage::from_pixel(width, height, background);

        // Top panel: the rectified photograph, with the sampled band marked
        // down the left edge so it is obvious which slice the grid came from.
        if let Some((preview, (band_top, band_bottom), _)) = &sample.preview {
            blit_scaled(&mut img, preview, BAND_MARK_PX, 0, preview_scale);
            let mark = image::Rgb([255, 204, 68]);
            for y in (band_top * preview_scale)..(band_bottom * preview_scale).min(img.height()) {
                for x in 0..BAND_MARK_PX {
                    img.put_pixel(x, y, mark);
                }
            }
        }

        // Middle panel: the block-resolution grids, one per wall.
        let mut y_offset = preview_h;
        for grid in &sample.grids {
            for row in 0..grid.rows {
                for col in 0..grid.cols {
                    let Some(color) = grid.cells[row * grid.cols + col] else {
                        continue;
                    };
                    let px = image::Rgb([color.0, color.1, color.2]);
                    for dy in 0..DEBUG_CELL_PX {
                        for dx in 0..DEBUG_CELL_PX {
                            img.put_pixel(
                                BAND_MARK_PX + col as u32 * DEBUG_CELL_PX + dx,
                                y_offset + row as u32 * DEBUG_CELL_PX + dy,
                                px,
                            );
                        }
                    }
                }
            }
            y_offset += (grid.rows as u32 + 1) * DEBUG_CELL_PX;
        }

        // Bottom bar: the resolved colour.
        let bar = image::Rgb([sample.color.0, sample.color.1, sample.color.2]);
        for y in height - DEBUG_CELL_PX..height {
            for x in 0..width {
                img.put_pixel(x, y, bar);
            }
        }

        let path = dir.join(format!("{}.png", sample.way_id));
        img.save(&path)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        written += 1;
    }
    let index_path = dir.join("previews.tsv");
    std::fs::write(&index_path, index)
        .map_err(|e| format!("write {}: {e}", index_path.display()))?;
    Ok(written)
}

/// Everything the pipeline needs from `Args`, owned, so the job can outlive the
/// borrow the caller had.
fn pipeline_config(args: &Args, llbbox: LLBBox) -> Option<pipeline::PipelineConfig> {
    let token = args.mapillary_api_token()?;
    let bbox = types::BBox::new(
        llbbox.min().lat(),
        llbbox.min().lng(),
        llbbox.max().lat(),
        llbbox.max().lng(),
    );
    let fetch = fetch::FetchConfig::new(token, bbox);
    let mut cfg = pipeline::PipelineConfig::new(fetch, types::Params::default());
    // The world build waits on this job, so a box whose cold run takes hours may
    // only be answered out of the cache. See `PRECOMPUTE_MAX_AREA_M2`.
    let area = bbox_area_m2(llbbox);
    if area > PRECOMPUTE_MAX_AREA_M2 {
        cfg.cache_only = Some(too_large_for_a_cold_run(area));
    }
    if let Some(dir) = args.mapillary_facade_debug_dir.clone() {
        cfg.debug = Some(pipeline::DebugDump {
            dir,
            walls: args
                .mapillary_facade_debug_walls
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        });
    }
    Some(cfg)
}

/// Runs the whole facade pipeline for this world and hands back the export
/// directory `facades::install` should load, or `None` when there is nothing to
/// load.
///
/// Every failure is reported once here and swallowed: Mapillary is an optional
/// enrichment, and neither a missing token nor an area with no imagery nor a
/// network outage may cost the user their world. Reporting it here rather than
/// at the wall is the difference between one line and one line per wall.
fn run_facade_pipeline(mut cfg: pipeline::PipelineConfig) -> Option<PathBuf> {
    // A generation that failed before collecting this job sets cancel and does
    // not wait, so this thread can still be winding down while the next
    // generation is under way. Its own progress lines would then land in that
    // generation's status bar, describing work nobody asked for. A cancelled
    // run reports to the terminal and stays out of the GUI.
    let cancel = cfg.cancel.clone();
    let abandoned = move || {
        cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Acquire))
    };
    let say = |msg: &str| {
        if !abandoned() {
            emit_gui_progress_update(MESSAGE_ONLY, &short_line(msg));
        }
    };
    // The Graph API has no title field, so the credit line names the place; see
    // the `fetch` module header. A reverse geocode is not worth failing over.
    let bbox = cfg.fetch.bbox;
    cfg.fetch.area_label = crate::retrieve_data::fetch_area_name(
        0.5 * (bbox.min_lat + bbox.max_lat),
        0.5 * (bbox.min_lon + bbox.max_lon),
    )
    .ok()
    .flatten();

    match pipeline::run(&cfg) {
        Ok(result) => {
            println!("  Mapillary facades: {}", result.stats.summary());
            // CC BY-SA is on the pixels. The GUI shows these under License and
            // Credits; a CLI run has nowhere else to put them.
            if !crate::progress::is_running_with_gui() {
                let used = credits::list();
                if !used.is_empty() {
                    println!("  Mapillary imagery used ({}):", used.len());
                    for credit in &used {
                        println!("    {}", credit.line());
                    }
                }
            }
            if result.stats.exported_walls == 0 {
                let (short, long) = if result.stats.images == 0 {
                    (
                        "Facades: no photos cover this area".to_string(),
                        "Mapillary facades: no street-level imagery covers this area".to_string(),
                    )
                } else {
                    (
                        "Facades: no wall is seen well enough".to_string(),
                        format!(
                            "Mapillary facades: {} images found, but no wall was seen well \
                             enough to texture",
                            result.stats.images
                        ),
                    )
                };
                println!("  {}", long.yellow());
                say(&short);
                return None;
            }
            say(&format!(
                "Facades: {} walls on {} buildings",
                result.stats.exported_walls, result.stats.exported_buildings
            ));
            // The run's own export directory, not a place the config names: two
            // generations at once each get their own and neither clears the
            // other's.
            Some(result.export_dir)
        }
        Err(e) => {
            eprintln!(
                "{} Mapillary facades skipped: {e}",
                "Warning:".yellow().bold()
            );
            // `say` keeps the first line of it, which is where the writers of
            // these put the part that fits a status line.
            say(&format!("Facades: {e}"));
            None
        }
    }
}

// --------------------------------------------------------------------------- precomputing an area

/// The largest box the pipeline will fetch imagery for, in square metres.
///
/// The Munich test box is 0.034 km2. Measured cold on this machine under the
/// lock on 2026-09-06, a precompute of it took **1499 s and 568 MB** of imagery
/// (align 1168 s of that, which is the registration of every photograph that
/// can see into the box, and texture 315 s). The brief's own figure of 215 s
/// and 3.1 GB is a warm one; this is what a first look at an area costs.
///
/// Both numbers follow the ground the box covers, so this cap, three times that
/// box, is about an hour. That is a great deal to ask of one button press, and
/// it is as far as it goes: the answer to a larger area is to precompute it in
/// pieces, because the per wall cache keeps everything each piece builds and no
/// piece redoes another's walls. A larger box is refused with its own size,
/// this one, and that advice, so the user knows what to do rather than guessing.
///
/// **Generation holds to the same cap**, and it used not to. The argument for
/// exempting it was that the pipeline runs beside the world build and every
/// failure of it is swallowed ([`run_facade_pipeline`]), so the worst a huge box
/// could cost was facades that never arrived. That was wrong about the wait:
/// `data_processing` calls [`FacadeJob::join`] before it builds a single
/// building, so the world stops there until the pipeline is done, and generation
/// has no cancel button. An ordinary 1 km2 selection is thirty times this cap on
/// ground and more than that in align work, which is photographs times walls, so
/// a user who turned the feature on and drew a normal Arnis box got a generation
/// parked on "Waiting for Mapillary facades..." for hours with a frozen progress
/// bar and no way out but killing the app, which costs them the world as well.
///
/// Above the cap, generation therefore runs the pipeline in cache-only mode
/// ([`pipeline::PipelineConfig::cache_only`]): an area already precomputed in
/// pieces still gets its facades, out of the per wall cache and with no
/// downloads, and an area that is not is told so in one Overpass query instead
/// of holding the world for an afternoon.
pub const PRECOMPUTE_MAX_AREA_M2: f64 = 100_000.0;

/// The part of a message the status line has room for.
///
/// The controls panel is 32 per cent of a 1000 pixel window, so its progress
/// line holds about fifty characters: a sentence longer than that wraps across
/// the panel and pushes the progress bar down it for as long as the message
/// shows. A message written for both places therefore puts the short form
/// first, then a blank line, then the detail, and this is the first part of it.
/// A message with no blank line is short already, and one that is neither, an
/// operating system error in some other language, is cut rather than left to
/// wrap.
fn short_line(message: &str) -> String {
    const MAX_CHARS: usize = 48;
    let head = message
        .split_once("\n\n")
        .map_or(message, |(head, _)| head)
        .trim();
    if head.chars().count() <= MAX_CHARS {
        return head.to_string();
    }
    let cut: String = head.chars().take(MAX_CHARS - 3).collect();
    format!("{}...", cut.trim_end())
}

/// What a generation over a box past [`PRECOMPUTE_MAX_AREA_M2`] is told when the
/// cache cannot answer it.
///
/// The way out exists and is the same one the Precompute button's own refusal
/// points at, so the short line spends its fifty characters on that rather than
/// on the numbers, which the terminal gets along with the reason for them. See
/// [`short_line`] for the two part shape.
fn too_large_for_a_cold_run(area_m2: f64) -> String {
    format!(
        "area too large, precompute it in pieces\n\n\
         This area is {:.2} km² and the facade pipeline only fetches imagery for boxes up to \
         {:.2} km², since the world build waits for it and a box this size takes hours from \
         cold. The cache does not hold all of this area yet, so no facades were built. \
         Precompute it in pieces (Settings, Mapillary, Precompute) and generate again: the \
         cache keeps every wall each piece builds and a generation then uses them without \
         downloading anything.",
        area_m2 / 1e6,
        PRECOMPUTE_MAX_AREA_M2 / 1e6,
    )
}

/// Where the finished walls of the current tunables live.
///
/// The one place a caller outside this module can name the facade tree, which
/// is what the Precompute button tells the user it filled.
pub fn facade_cache_dir() -> PathBuf {
    cache::Layout::default().facade_dir(&types::Params::default())
}

/// Ground area of a box in square metres.
pub fn bbox_area_m2(bbox: LLBBox) -> f64 {
    let mid = 0.5 * (bbox.min().lat() + bbox.max().lat());
    let height = (bbox.max().lat() - bbox.min().lat()).to_radians() * EARTH_RADIUS_M;
    // With the cosine, unlike the GUI's own selection readout: a box in Munich
    // reads a third wider than the ground without it, and this number is what
    // the cap above is measured in.
    let width = (bbox.max().lng() - bbox.min().lng()).to_radians()
        * EARTH_RADIUS_M
        * mid.to_radians().cos();
    (width * height).abs()
}

/// What one precompute did, in the terms the user is told it in.
#[derive(Clone, Debug, Default)]
pub struct PrecomputeReport {
    /// Buildings and walls that came out carrying a facade, which is what a
    /// world built over this box would get.
    pub buildings: usize,
    pub walls: usize,
    /// How many of those walls the cache already held, and how many this run
    /// had to build. They sum to `walls`.
    pub from_cache: usize,
    pub built: usize,
    /// Every wall in the area, facade or not. Most of a city's walls face a
    /// courtyard or a neighbour and no photograph ever sees them.
    pub examined: usize,
    pub images: usize,
    pub downloaded: usize,
    pub megabytes: f64,
    pub seconds: f64,
    /// Where the walls went, for the line that says so.
    pub cache_dir: PathBuf,
}

impl PrecomputeReport {
    /// True when the cache answered the whole area and nothing was fetched.
    pub fn all_cached(&self) -> bool {
        self.built == 0 && self.downloaded == 0
    }

    /// The one line the settings row shows.
    ///
    /// The three ways of coming back with nothing are told apart, because they
    /// ask for three different things of the user: another area, a different
    /// area, and nothing at all. Saying only "no facades" would leave someone
    /// over a park waiting for a download that was never going to happen.
    pub fn summary(&self) -> String {
        let took = format_seconds(self.seconds);
        if self.examined == 0 {
            return format!("Nothing to precompute: no OpenStreetMap buildings here ({took}).");
        }
        if self.walls == 0 {
            return if self.images == 0 {
                format!("Nothing to precompute: no street-level imagery covers this area ({took}).")
            } else {
                format!(
                    "Nothing to precompute: {} images cover this area, but none of its {} walls \
                     is seen well enough to texture ({took}).",
                    self.images, self.examined
                )
            };
        }
        if self.all_cached() {
            return format!(
                "Already precomputed: {} buildings and {} walls, all from the cache ({took}).",
                self.buildings, self.walls
            );
        }
        format!(
            "Precomputed {} buildings and {} walls in {took}: {} built now, {} already cached.",
            self.buildings, self.walls, self.built, self.from_cache
        )
    }

    /// The long version, for the row's tooltip: what it cost and where it went.
    pub fn detail(&self) -> String {
        format!(
            "{} walls examined, {} images found, {} downloaded ({:.0} MB). Stored in {}, so a \
             generation over this area now builds these walls without downloading anything. \
             Clearing the tile cache removes them.",
            self.examined,
            self.images,
            self.downloaded,
            self.megabytes,
            self.cache_dir.display()
        )
    }
}

/// `12s` under a minute, `1m 23s` above it. Minutes all the way up, because
/// this cap is ten of them and an hour would mean something had gone wrong.
fn format_seconds(seconds: f64) -> String {
    let whole = seconds.round().max(0.0) as u64;
    if whole < 60 {
        return format!("{whole}s");
    }
    format!("{}m {:02}s", whole / 60, whole % 60)
}

/// Fetches the imagery for one box and runs the whole pipeline over it, so the
/// walls land in the cache before any world asks for them.
///
/// This is the Precompute button. It is the same [`pipeline::run`] a generation
/// drives, on the same cache under the same tunables, so a generation over this
/// box afterwards finds every wall already built and does no image work at all.
/// It writes an export as well as the per wall entries, which is what lets the
/// 3D preview show the area (`facades::preview_walls_from_cache`).
///
/// Errors come back rather than being swallowed the way a generation's do: the
/// user pressed a button and is owed an answer, where a generation must not
/// lose its world to an optional enrichment.
pub fn precompute(
    llbbox: LLBBox,
    token: &str,
    cancel: Arc<AtomicBool>,
) -> Result<PrecomputeReport, String> {
    let bbox = types::BBox::new(
        llbbox.min().lat(),
        llbbox.min().lng(),
        llbbox.max().lat(),
        llbbox.max().lng(),
    );
    let mut cfg = pipeline::PipelineConfig::new(
        fetch::FetchConfig::new(token, bbox),
        types::Params::default(),
    );
    cfg.cancel = Some(cancel);
    // Same as a generation's run: the Graph API has no title field, so the
    // credit line names the place. Not worth failing over.
    cfg.fetch.area_label = crate::retrieve_data::fetch_area_name(
        0.5 * (bbox.min_lat + bbox.max_lat),
        0.5 * (bbox.min_lon + bbox.max_lon),
    )
    .ok()
    .flatten();

    let result = pipeline::run(&cfg)?;
    println!("  Mapillary precompute: {}", result.stats.summary());
    let stats = &result.stats;
    // Of the walls that came out with a facade, and not of the walls the
    // texture stage attempted: on this box it attempts every wall that has a
    // view and four fifths of those come out blank, so "built" counted that way
    // would be four times the number the user can see in the world.
    let from_cache = stats.cache_hits.min(stats.exported_walls);
    Ok(PrecomputeReport {
        buildings: stats.exported_buildings,
        walls: stats.exported_walls,
        from_cache,
        built: stats.exported_walls - from_cache,
        examined: stats.walls,
        images: stats.images,
        downloaded: stats.images_downloaded + stats.originals_downloaded,
        megabytes: stats.image_bytes as f64 / 1e6,
        seconds: stats.total_s,
        cache_dir: cfg.facade_cache.clone(),
    })
}

/// The facade pipeline running beside the rest of generation.
///
/// Downloads dominate the pipeline and the bbox is known long before anything
/// needs a facade, so the work starts as early as it can and is collected at
/// the one point that cannot go on without it, which is the buildings. The
/// thread is a plain `std` thread rather than a `rayon` task: it spends most of
/// its life in HTTP, and parking a pool worker there would cost the pipeline's
/// own `par_iter` stages the width they run on.
///
/// Clone, because `GenerationOptions` is, and joinable exactly once: whichever
/// clone gets there first takes the handle and the rest see it already gone.
#[derive(Clone, Default)]
pub struct FacadeJob {
    inner: Option<Arc<JobInner>>,
}

struct JobInner {
    handle: Mutex<Option<JoinHandle<Option<PathBuf>>>>,
    cancel: Arc<AtomicBool>,
}

impl FacadeJob {
    /// Starts the pipeline for this world, or hands back an idle job when
    /// there is nothing for it to do: the feature off, no token, a facade
    /// folder overriding the fetch, no buildings to put a facade on, or the
    /// coverage probe, which reports and exits before a world is built.
    ///
    /// Call it for every generation, idle or not. Clearing the attribution
    /// store is part of starting one, and a world that inherited the previous
    /// world's credit lines would be crediting imagery it does not carry.
    ///
    /// An idle job is not a world without imagery: the facade folder case
    /// builds from a finished export instead of fetching one, and the licence
    /// is on those pixels just the same. `facades::install` credits what that
    /// folder names, which is why clearing here is safe.
    pub fn start(args: &Args, llbbox: LLBBox) -> Self {
        credits::reset();
        if args.skip_objects() || args.mapillary_probe || !args.mapillary_pipeline_on() {
            return Self::default();
        }
        let Some(mut cfg) = pipeline_config(args, llbbox) else {
            return Self::default();
        };
        let cancel = Arc::new(AtomicBool::new(false));
        cfg.cancel = Some(Arc::clone(&cancel));
        let handle = std::thread::Builder::new()
            .name("mapillary-facades".to_string())
            .spawn(move || run_facade_pipeline(cfg));
        match handle {
            Ok(handle) => {
                emit_gui_progress_update(MESSAGE_ONLY, "Facades: starting...");
                Self {
                    inner: Some(Arc::new(JobInner {
                        handle: Mutex::new(Some(handle)),
                        cancel,
                    })),
                }
            }
            Err(e) => {
                eprintln!(
                    "{} Mapillary facades skipped: could not start the pipeline thread: {e}",
                    "Warning:".yellow().bold()
                );
                Self::default()
            }
        }
    }

    /// Whether a pipeline was actually started.
    pub fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    /// Waits for the pipeline and hands back the export directory.
    ///
    /// A panicked pipeline is reported and treated as no facades, for the same
    /// reason every other failure in here is: it must not cost the world.
    pub fn join(&self) -> Option<PathBuf> {
        let inner = self.inner.as_ref()?;
        let handle = inner
            .handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        if !handle.is_finished() {
            emit_gui_progress_update(MESSAGE_ONLY, "Waiting for facades...");
        }
        match handle.join() {
            Ok(dir) => dir,
            Err(_) => {
                eprintln!(
                    "{} Mapillary facades skipped: the pipeline thread panicked.",
                    "Warning:".yellow().bold()
                );
                None
            }
        }
    }
}

impl Drop for JobInner {
    /// A generation that failed before it reached the buildings will never
    /// collect this, so tell the pipeline to stop rather than leave it
    /// downloading imagery for a world nobody is going to get. It checks
    /// between stages and before every wall, so it winds down on its own; the
    /// handle is deliberately not joined here, because a caller unwinding out
    /// of a failure should not be made to wait on a download.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap is quoted to the user in km2, so it has to be measured on the
    /// ground the box covers and not on a plate carree rectangle: at Munich's
    /// latitude the two are a third apart, which is the difference between a
    /// box that is refused and one that is not.
    #[test]
    fn the_munich_test_box_measures_the_area_the_port_was_timed_on() {
        let munich = LLBBox::new(48.135635, 11.578243, 48.137225, 11.580818).unwrap();
        let area = bbox_area_m2(munich);
        assert!(
            (area - 33_800.0).abs() < 500.0,
            "the box the 215 s and 3.1 GB were measured on is 0.034 km2, got {area}"
        );
        assert!(
            area < PRECOMPUTE_MAX_AREA_M2,
            "and the cap has to admit it, or nothing can be precomputed"
        );
        // Three times that box is the cap, so a box four times it is refused.
        let wide = LLBBox::new(48.135635, 11.578243, 48.140000, 11.590000).unwrap();
        assert!(bbox_area_m2(wide) > PRECOMPUTE_MAX_AREA_M2);
    }

    /// Every line the button can end on. Coming back with nothing has three
    /// causes that ask three different things of the user, so they are told
    /// apart rather than reported alike.
    #[test]
    fn the_report_says_which_ending_this_was() {
        let base = PrecomputeReport {
            buildings: 39,
            walls: 111,
            examined: 1030,
            images: 469,
            seconds: 215.0,
            cache_dir: PathBuf::from("cache"),
            ..PrecomputeReport::default()
        };

        let built = PrecomputeReport {
            built: 111,
            downloaded: 469,
            megabytes: 247.0,
            ..base.clone()
        };
        assert!(!built.all_cached());
        let line = built.summary();
        assert!(line.contains("39 buildings and 111 walls"), "{line}");
        assert!(line.contains("3m 35s"), "{line}");
        assert!(line.contains("111 built now, 0 already cached"), "{line}");
        assert!(
            built.detail().contains("1030 walls examined"),
            "{}",
            built.detail()
        );

        let cached = PrecomputeReport {
            from_cache: 111,
            seconds: 1.6,
            ..base.clone()
        };
        assert!(cached.all_cached());
        let line = cached.summary();
        assert!(line.starts_with("Already precomputed:"), "{line}");
        assert!(line.contains("2s"), "{line}");

        // Buildings, but no photograph anywhere near them.
        let no_imagery = PrecomputeReport {
            buildings: 0,
            walls: 0,
            images: 0,
            ..base.clone()
        };
        let line = no_imagery.summary();
        assert!(line.contains("no street-level imagery covers"), "{line}");

        // Buildings and imagery, but nothing the pipeline could use: a
        // different sentence, because the answer is not to try elsewhere.
        let no_facades = PrecomputeReport {
            buildings: 0,
            walls: 0,
            ..base.clone()
        };
        let line = no_facades.summary();
        assert!(line.contains("469 images cover this area"), "{line}");
        assert!(line.contains("none of its 1030 walls"), "{line}");

        // No buildings at all, which is a box over a park or a field.
        let no_buildings = PrecomputeReport {
            buildings: 0,
            walls: 0,
            examined: 0,
            ..base
        };
        let line = no_buildings.summary();
        assert!(line.contains("no OpenStreetMap buildings"), "{line}");
    }

    /// The Precompute button end to end, on the real network and the user's
    /// real cache, with the preview reading back what it left.
    ///
    /// Ignored because it is neither: it talks to Overpass and to Mapillary,
    /// it writes into `dirs::cache_dir()`, and a cold run over this box is
    /// twenty minutes. It is the only test that proves the three pieces meet,
    /// so it is kept and run by hand:
    ///
    ///     cargo test --features gui -- --ignored --nocapture \
    ///         precompute_fills_the_cache_and_the_preview_reads_it_back
    ///
    /// The token is Mapillary's own public demo token, from their MIT
    /// `api-demo` repository.
    #[test]
    #[ignore = "network, the user's cache, and twenty minutes cold"]
    fn precompute_fills_the_cache_and_the_preview_reads_it_back() {
        const MUNICH: &str = "48.135635,11.578243,48.137225,11.580818";
        const DEMO_TOKEN: &str = "MLY|26275324248758064|7819d63bee8179a083cdd76e20557967";

        let bbox = LLBBox::from_str(MUNICH).unwrap();
        assert!(bbox_area_m2(bbox) < PRECOMPUTE_MAX_AREA_M2);

        let cancel = Arc::new(AtomicBool::new(false));
        let report = precompute(bbox, DEMO_TOKEN, cancel).expect("the precompute must finish");
        println!("summary: {}", report.summary());
        println!("detail:  {}", report.detail());
        assert!(report.walls > 0, "the Munich test box carries facades");
        assert!(report.buildings > 0);

        // And the preview, which is the other half of the button: the same
        // cache, read back through the path the 3D view uses.
        let json = facades::preview_walls_from_cache(MUNICH).expect("the preview must read back");
        let walls: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        println!("preview: {} walls", walls.len());
        assert!(
            !walls.is_empty(),
            "the preview must show what the precompute just built"
        );
        assert!(walls[0]["tex"]
            .as_str()
            .unwrap_or_default()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn offset_point_walks_the_compass() {
        let (lat, lon) = (52.5, 13.4);

        let north = offset_point(lat, lon, 0.0, 1000.0).unwrap();
        assert!(north.lat() > lat, "north should raise latitude");
        assert!(
            (north.lng() - lon).abs() < 1e-6,
            "north should hold longitude"
        );

        let east = offset_point(lat, lon, 90.0, 1000.0).unwrap();
        assert!(east.lng() > lon, "east should raise longitude");
        assert!(
            (east.lat() - lat).abs() < 1e-4,
            "east should roughly hold latitude"
        );

        let south = offset_point(lat, lon, 180.0, 1000.0).unwrap();
        assert!(south.lat() < lat);
    }

    #[test]
    fn offset_distance_is_about_right() {
        let (lat, lon) = (52.5, 13.4);
        let north = offset_point(lat, lon, 0.0, 1000.0).unwrap();
        // One km of latitude is close to 0.009 degrees anywhere on earth.
        let delta_deg = north.lat() - lat;
        assert!((delta_deg - 0.00899).abs() < 1e-4, "delta={delta_deg}");
    }

    /// An `Args` with everything off, so a case can turn on the one field it is
    /// about.
    fn bare_args() -> Args {
        use clap::Parser;
        Args::parse_from(["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"].iter())
    }

    #[test]
    fn an_idle_job_joins_to_nothing() {
        let mut args = bare_args();
        args.mapillary_token = None;
        let bbox = LLBBox::from_str("48.1,11.5,48.2,11.6").unwrap();
        let job = FacadeJob::start(&args, bbox);
        assert!(!job.is_running(), "no token means no pipeline");
        assert_eq!(job.join(), None);
    }

    #[test]
    fn a_facade_folder_keeps_the_pipeline_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut args = bare_args();
        args.mapillary_token = Some("MLY|test".to_string());
        args.mapillary_facades_dir = Some(dir.path().to_path_buf());
        let bbox = LLBBox::from_str("48.1,11.5,48.2,11.6").unwrap();
        // The folder is the export, so nothing may be fetched for this world.
        assert!(!FacadeJob::start(&args, bbox).is_running());
    }

    /// The cap reaches generation and not only the Precompute button.
    ///
    /// The world build waits on `FacadeJob::join`, so a box whose cold run takes
    /// hours is a generation with no visible end and no cancel. Above the cap the
    /// job may only answer out of the cache; below it nothing changes, which is
    /// what keeps the Munich test box and every fixture on the ordinary path.
    /// The status line gets the first paragraph, and never more than a line of
    /// it.
    #[test]
    fn the_status_line_takes_the_first_paragraph_and_no_more_than_a_line() {
        assert_eq!(short_line("short enough"), "short enough");
        assert_eq!(
            short_line("the way out\n\nAnd every number behind it, at length."),
            "the way out"
        );
        // Counted in characters and not bytes, or a message in a language with
        // accents on it would be cut in the middle of one.
        let cut = short_line(&"uberlang".replace('u', "\u{fc}").repeat(20));
        assert_eq!(cut.chars().count(), 48);
        assert!(cut.ends_with("..."), "{cut}");
    }

    #[test]
    fn a_generation_box_past_the_cap_may_only_answer_from_the_cache() {
        let mut args = bare_args();
        args.mapillary_token = Some("MLY|test".to_string());

        // The Munich test box: 0.034 km2, well under the cap, cold path as ever.
        let munich = LLBBox::from_str("48.135635,11.578243,48.137225,11.580818").unwrap();
        assert!(bbox_area_m2(munich) < PRECOMPUTE_MAX_AREA_M2);
        let cfg = pipeline_config(&args, munich).expect("a token is set");
        assert!(cfg.cache_only.is_none(), "a small box still fetches");

        // An ordinary Arnis selection, about 1 km2, is thirty times the cap.
        let big = LLBBox::from_str("48.130000,11.570000,48.139000,11.583400").unwrap();
        let area = bbox_area_m2(big);
        assert!(
            area > PRECOMPUTE_MAX_AREA_M2,
            "{area} m2 must be over the cap"
        );
        let cfg = pipeline_config(&args, big).expect("a token is set");
        let why = cfg.cache_only.expect("a box this size may not fetch");
        // The first line is all the status line shows, so it has to fit one and
        // to carry the way out on its own; the numbers and the reason behind
        // them are in the rest, which the terminal gets.
        let head = short_line(&why);
        assert!(head.chars().count() <= 48, "{head}");
        assert!(head.contains("pieces"), "{head}");
        assert!(!head.ends_with("..."), "the first line was cut: {head}");
        assert!(why.contains("km²"), "{why}");
        assert!(why.contains("Precompute"), "{why}");
    }

    #[test]
    fn a_job_is_joined_once_however_many_clones_hold_it() {
        // No token, so nothing runs and both joins are the idle answer; the
        // point is that the second one is not a panic on an already-taken
        // handle, which is what the two `generate_world_with_options` call
        // sites in the GUI would hit.
        let mut args = bare_args();
        args.mapillary_token = None;
        let bbox = LLBBox::from_str("48.1,11.5,48.2,11.6").unwrap();
        let job = FacadeJob::start(&args, bbox);
        let clone = job.clone();
        assert_eq!(job.join(), None);
        assert_eq!(clone.join(), None);
    }
}
