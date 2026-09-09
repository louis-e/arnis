//! The driver that runs the stages in order. Port of `tools/facade_lab/run.py`
//! and of `tools/facade_lab/export.py`'s `texture` stage.
//!
//! Stage order, which is also the order the port was written in:
//! [`super::fetch`], [`super::geometry`] with [`super::pose`] and
//! [`super::sfm`], then align ([`super::register`] over [`super::visibility`]
//! and [`super::plane`]), then texture ([`super::rectify`], [`super::refine`],
//! [`super::fuse`], [`super::openings`], [`super::bands`],
//! [`super::confidence`]), then the export.
//!
//! Three properties this driver has to keep:
//!
//! * **Walls are independent after registration.** That is what lets `rayon`
//!   loop over walls, and it is also why no gate may become a statistic of the
//!   run as invoked: a per-run blur reference was measured and rejected partly
//!   because it made the same building yield different facades depending on
//!   where the bbox was drawn.
//! * **Downloads dominate, so nothing is downloaded on spec, and nothing the
//!   run will read is left undownloaded either.** The image list is decided by
//!   the gates inside [`stage_align`], from the **registered** camera centres,
//!   which is the earliest point it can be decided correctly: registration
//!   moves a camera by up to 11.8 m on the Munich box, so the same gates run on
//!   the raw Graph poses ask a different question. On that box the answer is
//!   470 thumbnails of 1076 (246 MB of 555), plus the 218 full resolution
//!   originals the `hires` ladder in the texture stage will actually read.
//!
//!   The alternatives were measured against the reference run's 111 view lists.
//!   Downloading the whole box reproduces 82 of them exactly and leaves no wall
//!   without a view; the 470 reproduce the same 82 and the same zero. The
//!   prefilter this replaced, the same gates against the raw poses, downloaded
//!   450 and reproduced 67, emptying 6 walls; opening its distance bounds by a
//!   margin does not converge, because a registration changes incidence,
//!   angular width and line of sight and not only range: at 12 m it downloads
//!   502 for 71, and at 25 m it downloads 546, more than deferring costs, and
//!   still reproduces only 71 and still empties 5. The one margined prefilter
//!   that is actually sound, "any camera within range of any wall", takes all
//!   1076 on this box, so it is option one under another name.
//! * **Never hold more than one wall's views in memory at once.** The fetch
//!   stage hands back paths, and a wall decodes its own three photographs
//!   inside its own `rayon` task and drops them with it.
//!
//! The `rayon` width is the crate's own global pool, the one
//! `floodfill_cache::configure_rayon_thread_pool` sizes at 90 per cent of the
//! cores; only the downloads get a pool of their own, in [`super::fetch`],
//! because their limit is connections rather than cores.
//!
//! The output is the same export format `tools/facade_lab/export_arnis.py`
//! writes, put in the cache, and `facades::install` loads it as it already
//! does. One format and one consumer: the block and photo modes keep working
//! untouched, the cache is the export so reuse and clearing are the same
//! mechanism, and a golden test is a directory comparison against the Python
//! output.
//!
//! Two trees under `arnis-tile-cache/mapillary/facades/<params digest>/`, and
//! the split is the difference between what survives a new bbox and what does
//! not:
//!
//! * `<shard>/<wall digest>.{json,png,_tex.png}` is the reuse cache, keyed by
//!   the wall's OSM node ids, so a wall already built is not built again
//!   whatever bbox the next generation uses. A wall proved to carry no facade
//!   is filed there too, so that answer is reusable per wall as well;
//! * `exports/<run>/` is one run's export, written once and handed to
//!   `facades::install`. It is derived entirely from the reuse cache plus the
//!   geometry, so a run that hits the cache on every wall does no image work
//!   at all and still writes a complete export.
//!
//! **A run never writes into another run's export.** The directory carries a
//! nonce and is claimed with `create_dir`, so two generations at once cannot
//! land in one, and no run ever deletes a directory another one might be
//! reading. That was not always true: one `export/` per digest, cleared at the
//! start of every write, lost 2 of 12 concurrent pairs their facades and left
//! the owner's cache with a manifest claiming 0 buildings beside 407 files from
//! a different run. [`sweep_exports`] takes the old ones out afterwards, and
//! only ones old enough that nothing can still be reading them.
//!
//! Two differences from the Python worth writing down.
//!
//! `export.py` classified each block with `blocks.quantise` and only
//! `export_arnis.py` afterwards rebuilt the wall as floor bands. The port
//! dropped `blocks.py`, so the block product here is `openings::classify` plus
//! `bands::analyse`, which is what `export_arnis.py` writes and what the golden
//! fixtures pin. The one thing lost with `blocks.py` is its `observed` mask,
//! whose no-data rule counted a cell's partially off-image texels; here
//! `observed` is `class != no data`, which differs on 212 of 12276 fixture
//! cells over 4 of 45 walls. `blocks.fuse_block_grids` goes with it: on a wall
//! whose views disagree by more than `fuse::AGREEMENT_MAX_M` the Python
//! quantised each view and combined the classes, where this classifies the best
//! view's texture, which is the texture `fuse` hands back in that mode anyway.
//!
//! The Python driver never called `refine::finalise_wall`, so the reference run
//! carries no colour trim on the ends OSM decided, and neither does this. The
//! function is ported and tested; turning it on is a change to the pipeline,
//! not a port of it.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use image::{RgbImage, RgbaImage};
use rayon::prelude::*;
use serde_json::{json, Map, Value};

use super::bands::{self, CellSource, Structure, BAND_TAU};
use super::cache::{self, ImageSize, Layout};
use super::confidence;
use super::credits::{self, ImageCredit};
use super::fetch::{Batch, Credit, FetchConfig, Fetched};
use super::fuse::{self, ViewTexture};
use super::geometry;
use super::imgops;
use super::openings::{WallTexture, CLS_NODATA, CLS_UNKNOWN};
use super::plane;
use super::rectify::{self, LooseCrop, View};
use super::refine::{self, Refinement, ViewEvidence, ViewInputs};
use super::register::{self, Align, AlignInput, WallViews};
use super::sfm::{self, Cluster};
use super::types::{
    Building, Camera, Frame, GateName, PanoMeta, Params, PlaneFit, Tier, Wall, WallDecision,
    WallEdge, WallProduct,
};
use super::{fetch, pose};
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};

/// `hires` in the Python (`export.py`'s `HIRES_MAX_DIST_M`, same value): a view
/// nearer than this renders from the full resolution original rather than the
/// 2048 px thumbnail.
///
/// The Python's `prefetch.py --all-panos` put an original on disk for every
/// image in the box and `pose.load_pano` then picked one up whenever
/// `export.py` asked for it, so the reference run textured its near views from
/// 5760x2880 pixels where the port textured them from 2048x1024. That is a
/// resampling ratio of nearly three, and it moved the rectangle on 39 of 105
/// shared walls on its own. [`fetch_originals`] downloads exactly the originals
/// this run will read, which is a small fraction of the box: the ladder is only
/// ever consulted for a **selected** view of a wall that is actually being
/// built.
const HIRES_MAX_DIST_M: f64 = 30.0;

/// OkLab `a` below this is vegetation and never contributes a building colour.
const GREEN_A: f64 = -0.04;
/// OkLab `b` below this with `L` above 0.6 is sky.
const SKY_B: f64 = -0.05;
/// Building colour confidence is `1 - MAD / this` in sRGB units. `DESIGN.md`
/// says 25; the shading of one sunlit facade alone gives a MAD of 20 to 25 on
/// the Munich cache, which would zero every tier A building.
const COLOUR_MAD_SCALE: f64 = 50.0;

// --------------------------------------------------------------------------- configuration

/// Where to dump one wall's intermediate products, so the Python review figures
/// can be rendered from a Rust run.
#[derive(Clone, Debug, Default)]
pub struct DebugDump {
    pub dir: PathBuf,
    /// Wall keys to dump. Empty dumps every wall the texture stage builds,
    /// which is a few hundred megabytes on a city and is why it is opt in.
    pub walls: Vec<String>,
}

impl DebugDump {
    fn wants(&self, wall_key: &str) -> bool {
        self.walls.is_empty() || self.walls.iter().any(|w| w == wall_key)
    }
}

/// One run of the pipeline.
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub fetch: FetchConfig,
    pub params: Params,
    /// Where the finished walls are cached and read back from.
    pub facade_cache: PathBuf,
    /// Walls are independent, so this is a plain `rayon` width. Zero uses the
    /// pool `floodfill_cache::configure_rayon_thread_pool` already built, which
    /// is what every other CPU stage in the crate runs on.
    pub threads: usize,
    /// Set by a caller that wants the run to stop. Checked between stages and
    /// before every wall. It does **not** reach inside `fetch`, so a run
    /// cancelled while the imagery is downloading finishes that download first;
    /// the same goes for the align stage, which is one call. Generation has no
    /// stop button today, so the only thing that sets this is `FacadeJob`'s
    /// `Drop`, on a generation that failed before it reached the buildings.
    pub cancel: Option<Arc<AtomicBool>>,
    /// When set, this run may only answer out of the facade cache: if the cache
    /// does not already hold every wall of the area, it stops here and this is
    /// the sentence the user is told, rather than fetching imagery.
    ///
    /// The caller that sets it is `FacadeJob::start`, on a box too large for the
    /// cold path to finish in a time anybody would wait ([`crate::mapillary::PRECOMPUTE_MAX_AREA_M2`]).
    /// It is not a cancel: a run that the cache can answer produces a full
    /// export and the world gets its facades as usual.
    pub cache_only: Option<String>,
    pub debug: Option<DebugDump>,
}

impl PipelineConfig {
    /// The facade tree for these tunables, under the cache the fetch config
    /// names.
    pub fn new(fetch: FetchConfig, params: Params) -> Self {
        let facade_cache = Layout::new(fetch.cache_dir.clone()).facade_dir(&params);
        Self {
            fetch,
            params,
            facade_cache,
            threads: 0,
            cancel: None,
            cache_only: None,
            debug: None,
        }
    }

    /// Where this run's own export directory is minted, one per run.
    ///
    /// Not one directory per digest: that one was shared by every generation
    /// and cleared at the start of every write, so two runs at once overwrote
    /// each other and a run that found nothing wiped a good export. A run mints
    /// its own below and hands the path back in [`PipelineResult::export_dir`].
    pub fn exports_dir(&self) -> PathBuf {
        cache::exports_root(&self.facade_cache)
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            return Err("cancelled".to_string());
        }
        Ok(())
    }
}

/// How long each stage took and how much of the run the cache answered.
#[derive(Clone, Debug, Default)]
pub struct RunStats {
    pub images: usize,
    /// Thumbnails the align stage asked for, and originals the texture stage
    /// asked for. Both counts are of a whole set: what a first run over the
    /// area downloads, and what a later one finds already cached.
    pub images_downloaded: usize,
    pub originals_downloaded: usize,
    /// What those two sets weigh on disk.
    pub image_bytes: u64,
    pub clusters: usize,
    pub buildings: usize,
    pub walls: usize,
    pub reachable: usize,
    pub walls_with_views: usize,
    /// Panos registered locally, by the cluster-wide fallback, and not at all.
    pub reg_local: usize,
    pub reg_global: usize,
    pub reg_none: usize,
    /// Walls the facade cache answered without any image work.
    pub cache_hits: usize,
    /// Walls the texture stage had to build.
    pub cache_misses: usize,
    /// Counts in tier order A, B, C, D.
    pub tiers: [usize; 4],
    pub exported_buildings: usize,
    pub exported_walls: usize,
    /// Images whose pixels reached a wall and so have to be credited.
    pub credited: usize,
    pub fetch_s: f64,
    pub geometry_s: f64,
    pub align_s: f64,
    /// The originals round trip, which sits between align and texture.
    pub hires_s: f64,
    pub texture_s: f64,
    pub export_s: f64,
    pub total_s: f64,
}

impl RunStats {
    /// The one line a run prints, in the shape `run.py` prints its stages in.
    pub fn summary(&self) -> String {
        format!(
            "{} buildings, {} walls ({} reachable, {} with views); \
             reg {} local / {} global / {} none; tiers A {} / B {} / C {} / D {}; \
             cache {} hit / {} built; {} thumbnails and {} originals ({:.0} MB); \
             {} buildings and {} walls exported, {} images credited; \
             fetch {:.0} s, geometry {:.0} s, align {:.0} s, hires {:.0} s, texture {:.0} s, export {:.0} s, total {:.0} s",
            self.buildings,
            self.walls,
            self.reachable,
            self.walls_with_views,
            self.reg_local,
            self.reg_global,
            self.reg_none,
            self.tiers[0],
            self.tiers[1],
            self.tiers[2],
            self.tiers[3],
            self.cache_hits,
            self.cache_misses,
            self.images_downloaded,
            self.originals_downloaded,
            self.image_bytes as f64 / 1e6,
            self.exported_buildings,
            self.exported_walls,
            self.credited,
            self.fetch_s,
            self.geometry_s,
            self.align_s,
            self.hires_s,
            self.texture_s,
            self.export_s,
            self.total_s
        )
    }
}

/// What a run produced.
#[derive(Clone, Debug)]
pub struct PipelineResult {
    /// The ENU frame every coordinate in here is measured in.
    pub frame: Frame,
    pub buildings: Vec<Building>,
    pub walls: Vec<Wall>,
    pub products: Vec<WallProduct>,
    /// Every image that contributed a pixel, for License and Credits.
    pub credits: Vec<Credit>,
    /// This run's own export, which is what `facades::install` is pointed at.
    /// Empty on a result that never wrote one.
    pub export_dir: PathBuf,
    pub stats: RunStats,
}

impl Default for PipelineResult {
    fn default() -> Self {
        Self {
            frame: Frame::new(0.0, 0.0),
            buildings: Vec::new(),
            walls: Vec::new(),
            products: Vec::new(),
            credits: Vec::new(),
            export_dir: PathBuf::new(),
            stats: RunStats::default(),
        }
    }
}

// --------------------------------------------------------------------------- the geometry stage

/// What the geometry stage produced and every later stage reads.
pub struct Geometry {
    pub frame: Frame,
    pub buildings: Vec<Building>,
    pub walls: Vec<Wall>,
    pub cameras: BTreeMap<String, Camera>,
    pub metas: BTreeMap<String, PanoMeta>,
    /// Loaded once here because the align stage and the wall plane both want
    /// them, and one of these is tens of megabytes.
    pub clusters: HashMap<String, Cluster>,
    /// `(xmin, ymin, xmax, ymax)` of the run bbox in the run frame.
    pub bbox_xy: [f64; 4],
}

/// ENU frame, footprints, walls and cameras. `geo.py`, `pose.py` and `sfm.py`.
pub fn stage_geometry(cfg: &PipelineConfig, fetched: &Fetched) -> Result<Geometry, String> {
    cfg.check()?;
    let params = &cfg.params;
    let (frame, buildings, walls) = footprints_and_walls(cfg, &fetched.osm);
    if buildings.is_empty() {
        return Err("no buildings in the area".to_string());
    }

    let metas: Vec<PanoMeta> = fetched
        .metas
        .iter()
        .filter(|m| params.admits(m.camera_type))
        .cloned()
        .collect();
    let mut by_seq: HashMap<&str, Vec<&PanoMeta>> = HashMap::new();
    for m in &metas {
        by_seq.entry(m.sequence.as_str()).or_default().push(m);
    }
    let meta_map: HashMap<&str, &PanoMeta> = metas.iter().map(|m| (m.id.as_str(), m)).collect();
    let empty: Vec<&PanoMeta> = Vec::new();
    let mut cameras: HashMap<String, Camera> = metas
        .iter()
        .map(|m| {
            let seq = by_seq.get(m.sequence.as_str()).unwrap_or(&empty);
            (
                m.id.clone(),
                pose::camera_from_meta(m, &frame, seq, params, None),
            )
        })
        .collect();

    // The clusters carry the shot poses and the point cloud, so they have to be
    // on disk before a camera is worth anything.
    cfg.check()?;
    let batch = fetch::download_clusters(&cfg.fetch, &fetched.clusters);
    for (id, why) in &batch.failed {
        eprintln!("Note: Mapillary cluster {id} not available ({why})");
    }
    let mut clusters: HashMap<String, Cluster> = HashMap::new();
    // Parsing 88 clusters is 675 MB of JSON on the Munich box, which is the
    // single slowest part of the geometry stage and is per file independent.
    let ready: Vec<(String, PathBuf)> = batch.ready.into_iter().collect();
    let parsed: Vec<(String, Option<Cluster>)> = in_pool(cfg.threads, || {
        ready
            .par_iter()
            .map(|(id, path)| {
                let cluster = cache::read_cached(path)
                    .and_then(|bytes| fetch::parse_cluster_bytes(&bytes).ok())
                    .and_then(|doc| sfm::load_cluster(&doc, id, &frame).ok());
                // A cached file that will not parse is never re-downloaded,
                // because `ensure_cluster` takes any file on disk as the
                // answer, so it would cost this cluster's cameras their poses
                // on every later run too. Same rule as `decode` applies to a
                // JPEG that will not open.
                if cluster.is_none() {
                    eprintln!(
                        "Note: Mapillary cluster {id} did not parse; {} removed",
                        path.display()
                    );
                    let _ = std::fs::remove_file(path);
                }
                (id.clone(), cluster)
            })
            .collect()
    });
    for (id, cluster) in parsed {
        cfg.check()?;
        // Reported and removed above, where the parse actually failed.
        let Some(mut cluster) = cluster else {
            continue;
        };
        let of_cluster: Vec<&PanoMeta> = metas
            .iter()
            .filter(|m| m.cluster_id.as_deref() == Some(id.as_str()))
            .collect();
        let shot_map = sfm::map_shots(&cluster, &of_cluster, params);
        let _ = sfm::metric_check(&mut cluster, &of_cluster, &shot_map, &frame, params);
        sfm::apply_shot_poses(
            &mut cameras,
            &cluster,
            &shot_map,
            &meta_map,
            &by_seq,
            params,
        );
        clusters.insert(id, cluster);
    }
    sfm::camera_heights(&mut cameras, &meta_map, &clusters, params);

    let lo = frame.to_enu(cfg.fetch.bbox.min_lon, cfg.fetch.bbox.min_lat);
    let hi = frame.to_enu(cfg.fetch.bbox.max_lon, cfg.fetch.bbox.max_lat);
    Ok(Geometry {
        frame,
        buildings,
        walls,
        cameras: cameras.into_iter().collect(),
        metas: metas.into_iter().map(|m| (m.id.clone(), m)).collect(),
        clusters,
        bbox_xy: [lo[0], lo[1], hi[0], hi[1]],
    })
}

// --------------------------------------------------------------------------- the align stage

/// Registration, candidates, gates, view selection and the wall planes, with
/// the thumbnails the gates need downloaded in the middle of it.
///
/// The download sits inside the stage rather than in front of it because only
/// the stage knows the list. Registration moves a camera by up to 11.8 m on the
/// Munich box, so the gates answer differently before and after it: 57 of the
/// panos wanted here were not in the answer the raw Graph poses gave, and 37 of
/// that answer's panos were never wanted. A prefilter built on it left 44 of the
/// reference run's 111 walls with a different view list and 6 of them with none
/// at all, rejecting as `no_image` pixels that were in the cache and had simply
/// never been asked for. Registration reads point clouds only, so deferring the
/// download until after it costs the run nothing but the round trip it was
/// always going to make.
///
/// Hands back the batch as well as the align result: the driver needs the paths
/// for the texture stage, and needs to know which downloads failed, because a
/// wall that lost its views to a failed download has not been proved blank.
pub fn stage_align(cfg: &PipelineConfig, geo: &Geometry) -> (Align, Batch) {
    let input = AlignInput {
        buildings: &geo.buildings,
        walls: &geo.walls,
        cameras: &geo.cameras,
        metas: &geo.metas,
        bbox_xy: geo.bbox_xy,
        params: &cfg.params,
    };
    let cluster_of = |id: &str| -> Option<Cluster> { geo.clusters.get(id).cloned() };
    let batch: Mutex<Batch> = Mutex::new(Batch::default());
    let request = |wanted: &BTreeSet<String>| {
        let ids: Vec<String> = wanted.iter().cloned().collect();
        let got = fetch::download_images(&cfg.fetch, &ids);
        for (id, why) in &got.failed {
            eprintln!("Note: Mapillary image {id} not available ({why})");
        }
        *batch.lock().expect("no panic holds the batch") = got;
    };
    let image_of = |id: &str| -> Option<RgbImage> {
        let path = batch
            .lock()
            .expect("no panic holds the batch")
            .path(id)
            .cloned()?;
        decode(&path)
    };
    let align = register::run_align(&input, &cluster_of, &request, &image_of);
    (align, batch.into_inner().expect("no panic holds the batch"))
}

/// The full resolution originals this run's texture stage will actually read,
/// downloaded and handed back by pano id.
///
/// Three filters, and each of them is what keeps the bill small. Only a
/// **selected** view counts, not every candidate that reached the gates; only a
/// view under [`HIRES_MAX_DIST_M`], because that is the only case the ladder
/// consults an original at all; and only a wall the texture stage is going to
/// build, because a wall answered out of the facade cache never opens a
/// photograph. On the Munich box that is 218 originals, 401 MB, against the
/// 1076 and 1855 MB the Python's `prefetch.py --all-panos` put on disk so that
/// `pose.load_pano` could pick those same 218 out of them.
///
/// A failed original is not a failed view: [`WallContext::image_for`] falls
/// straight back to the thumbnail, which is what the Python does for any pano
/// whose original was never fetched.
fn fetch_originals(
    cfg: &PipelineConfig,
    geo: &Geometry,
    align: &Align,
) -> BTreeMap<String, PathBuf> {
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    for wall in &geo.walls {
        let Some(vrec) = align.views.get(&wall.key) else {
            continue;
        };
        if vrec.views.is_empty() {
            continue;
        }
        // The cheap half of what `stage_texture` will decide: an entry whose
        // two files are there is a wall this run will not open a photograph
        // for. A damaged entry that `load_wall` then refuses costs that one
        // wall its original and falls back to the thumbnail, which is the same
        // outcome a failed download has.
        let key = cache::wall_cache_key(&wall_node_ids(wall), wall.piece);
        if cache::wall_paths(&cfg.facade_cache, &key)
            .is_ok_and(|(json, png, _)| json.exists() && png.exists())
        {
            continue;
        }
        for v in &vrec.views {
            if v.dist_m < HIRES_MAX_DIST_M {
                wanted.insert(v.pano_id.clone());
            }
        }
    }
    let mut hires = cfg.fetch.clone();
    hires.size = ImageSize::Original;
    let ids: Vec<String> = wanted.into_iter().collect();
    let batch = fetch::download_images(&hires, &ids);
    for (id, why) in &batch.failed {
        eprintln!("Note: Mapillary original {id} not available ({why}); using the thumbnail");
    }
    batch.ready
}

/// How many bytes a set of cached files takes, which is the download this run
/// pays for the first time it sees an area.
fn bytes_on_disk<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> u64 {
    paths
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// One cached JPEG, decoded. A file that will not decode is deleted, because it
/// can only poison every later run.
fn decode(path: &Path) -> Option<RgbImage> {
    let bytes = cache::read_cached(path)?;
    match image::load_from_memory(&bytes) {
        Ok(img) => Some(img.to_rgb8()),
        Err(e) => {
            eprintln!("Note: {} did not decode ({e}); removed", path.display());
            let _ = std::fs::remove_file(path);
            None
        }
    }
}

// --------------------------------------------------------------------------- the texture stage

/// Everything one wall's task reads. Shared by reference across the `rayon`
/// loop, so nothing here may be mutated.
struct WallContext<'a> {
    cfg: &'a PipelineConfig,
    geo: &'a Geometry,
    align: &'a Align,
    /// Cached thumbnail paths by pano id, and the originals the `hires` ladder
    /// prefers for a near view.
    images: &'a BTreeMap<String, PathBuf>,
    originals: &'a BTreeMap<String, PathBuf>,
}

impl WallContext<'_> {
    /// The photograph one view renders from, following the Python's `hires`
    /// ladder: the original for a near view, the thumbnail otherwise.
    fn image_for(&self, pano_id: &str, dist_m: f64) -> Option<RgbImage> {
        if dist_m < HIRES_MAX_DIST_M {
            if let Some(path) = self.originals.get(pano_id) {
                if let Some(img) = decode(path) {
                    return Some(img);
                }
            }
        }
        self.images.get(pano_id).and_then(|p| decode(p))
    }
}

/// One view of one wall, carried from the crop to the final texture.
struct LoadedView {
    pano_id: String,
    score: f64,
    dist_m: f64,
    /// The view's own plane, which is the wall frame plane for the frame view.
    fit: PlaneFit,
    /// How far along the wall this view's start sits from the frame's.
    ds_m: f64,
    z_base: f64,
    /// The ground row correction this view measured.
    dz: f64,
    s_vis: [f64; 2],
    h_cloud: Option<f64>,
    crop: LooseCrop,
    refinement: Refinement,
}

/// Texture one wall from its selected views. Port of `export.py::texture_wall`.
///
/// The ladder is the Python's: loose crops, per view refinement, the wall
/// decision, per view resampling onto the decided rectangle, fusion, the block
/// phase, the block product, then confidence. A view whose photograph will not
/// load drops out; a wall that loses every view becomes a no-image record
/// rather than an error, because one wall must never take the stage down.
fn texture_wall(ctx: &WallContext, wall: &Wall) -> WallProduct {
    let cfg = ctx.cfg;
    let params = &cfg.params;
    let align = ctx.align;
    let empty = WallViews::default();
    let vrec = align.views.get(&wall.key).unwrap_or(&empty);
    if vrec.views.is_empty() {
        return no_view_product(wall, vrec);
    }
    let fit = match align.planes.get(&wall.key) {
        Some(f) => f.clone(),
        None => plane::fallback_plane(wall, false, None, None),
    };
    let ppb = params.tex_ppb;

    // ---- per view: photograph, loose crop, refinement
    let mut loaded: Vec<LoadedView> = Vec::new();
    let mut images: Vec<RgbImage> = Vec::new();
    for c in &vrec.views {
        let pid = &c.pano_id;
        let Some(cam) = align.cameras.get(pid) else {
            continue;
        };
        let Some(image) = ctx.image_for(pid, c.dist_m) else {
            continue;
        };
        let vkey = super::types::view_key(&wall.key, pid);
        // The frame view already is the wall frame plane; every other view
        // carries its own fit and the offset between the two (CRITIQUE A5).
        let is_frame = fit.pano_id.as_deref() == Some(pid.as_str());
        let fit_k = if is_frame {
            fit.clone()
        } else {
            align
                .per_view
                .get(&vkey)
                .cloned()
                .unwrap_or_else(|| fit.clone())
        };
        let ds_m = if is_frame {
            0.0
        } else {
            align
                .per_view_offsets
                .get(&vkey)
                .map(|o| o.ds_m)
                .unwrap_or(0.0)
        };
        let z_base = align
            .z_bases
            .get(&vkey)
            .map(|(z, _)| *z)
            .unwrap_or(cam.ground_z);
        let h_cloud = plane::cloud_height(&fit_k, z_base);
        let view = View {
            cam,
            fit: &fit_k,
            z_base,
            dist_m: Some(c.dist_m),
            s_vis: Some(c.s_vis),
        };
        let crop = rectify::loose_crop(
            wall,
            &view,
            &image,
            align.depths.get(pid),
            &ctx.geo.buildings,
            params,
        );
        let inputs = ViewInputs {
            wall,
            pano_id: pid,
            l_fit: plane::fitted_wall(wall, &fit_k).length,
            s_vis: Some((c.s_vis[0], c.s_vis[1])),
            h_cloud,
            ground_source: cam.ground_source,
        };
        let (refinement, _details) = refine::refine_view(&crop, &inputs, params);
        loaded.push(LoadedView {
            pano_id: pid.clone(),
            score: c.score,
            dist_m: c.dist_m,
            fit: fit_k,
            ds_m,
            // The crop's own base, which is the datum every height in it and in
            // the refinement is measured from.
            z_base: crop.z_base,
            dz: refinement.ground_dz,
            s_vis: c.s_vis,
            h_cloud,
            crop,
            refinement,
        });
        images.push(image);
    }
    if loaded.is_empty() {
        let mut rec = no_view_product(wall, vrec);
        rec.flags = merged(&rec.flags, &["NO_IMAGE".to_string()]);
        return rec;
    }

    // ---- the wall decision: the rectangle and the height, in the wall frame
    let (src_a, src_b) = align
        .corners
        .get(&wall.key)
        .copied()
        .unwrap_or((plane::EndSource::Osm, plane::EndSource::Osm));
    let evidence: Vec<ViewEvidence> = loaded
        .iter()
        .map(|v| ViewEvidence {
            refinement: &v.refinement,
            score: v.score,
            ds_m: v.ds_m,
            // CRITIQUE B16: the ground row moved the base, so the cloud height
            // has to be measured from the moved base too.
            h_cloud: v.h_cloud.map(|h| h - v.dz),
            crop: Some(&v.crop),
        })
        .collect();
    let mut dec = refine::decide_wall(
        wall,
        plane::fitted_wall(wall, &fit).length,
        src_a == plane::EndSource::CloudCorner,
        src_b == plane::EndSource::CloudCorner,
        &evidence,
        &[],
        params,
    );
    if !dec.h_used.is_finite() || dec.h_used < 1.0 {
        // The Python's fallback: the cloud's opinion where there is one, the
        // OSM height otherwise, and never outside what a building can be.
        let clouds: Vec<f64> = loaded.iter().filter_map(|v| v.h_cloud).collect();
        let h = if clouds.is_empty() {
            wall.height_osm.unwrap_or(params.default_height_m)
        } else {
            imgops::median(&clouds)
        };
        dec.h_used = h.clamp(3.0, 80.0);
        dec.height_source = "default".to_string();
    }
    let cols = (imgops::round_half_even(dec.s_r - dec.s_l) as i64).max(1) as u32;
    let rows = (imgops::round_half_even(dec.h_used) as i64).max(1) as u32;
    // The rectangle is a whole number of blocks wide and is centred on what the
    // decision asked for, so the rounding residual is half a block at each end
    // rather than a block at one of them.
    let s_c = 0.5 * (dec.s_l + dec.s_r);
    let s_l = s_c - 0.5 * f64::from(cols);
    let s_r = s_c + 0.5 * f64::from(cols);

    // One dz per wall (the median over the views' ground edges) is applied to
    // every view: a per view dz would move the views against each other by the
    // noise of the edge detector, which the fusion residual then has to absorb.
    let dz_wall = imgops::median(&loaded.iter().map(|v| v.dz).collect::<Vec<_>>());

    // ---- per view: the final texture on the decided rectangle
    let mut view_tex: Vec<ViewTexture> = Vec::new();
    for (v, image) in loaded.iter().zip(images.iter()) {
        let rect = [
            s_l - v.ds_m,
            s_r - v.ds_m,
            dz_wall,
            f64::from(rows) + dz_wall,
        ];
        let view = View {
            cam: &align.cameras[&v.pano_id],
            fit: &v.fit,
            z_base: v.z_base,
            dist_m: Some(v.dist_m),
            s_vis: Some(v.s_vis),
        };
        let out = rectify::resample_rect(
            wall,
            &view,
            image,
            rect,
            Some(&v.refinement.h_shear),
            ppb,
            Some(&v.crop),
            params,
        );
        view_tex.push(ViewTexture {
            pano_id: v.pano_id.clone(),
            rgb: out.rgb,
            valid: out.valid,
            score: v.score,
        });
    }

    // ---- fusion, the block phase and the block product
    let fused = fuse::fuse(&wall.key, &view_tex, ppb, params);
    let (dx, dy, bim) = refine::phase_search(&fused.rgb, &fused.valid, ppb as usize, params);
    let origin_px = (
        imgops::round_half_even(dx * f64::from(ppb)) as i32,
        imgops::round_half_even(dy * f64::from(ppb)) as i32,
    );
    dec.phase = [dx, dy];
    dec.bimodality = bim;
    let tex = WallTexture::new(
        fused.rgb.width() as usize,
        fused.rgb.height() as usize,
        fused.rgb.pixels().map(|p| p.0).collect(),
        fused.valid.clone(),
    );
    let structure = bands::analyse(
        CellSource::Texture {
            tex: &tex,
            origin_px,
        },
        rows as usize,
        cols as usize,
        None,
        BAND_TAU,
    );

    // ---- confidence and the tier
    let cells = (rows as usize) * (cols as usize);
    let unknown_share = match structure.openings.as_ref() {
        Some(op) => {
            op.cls
                .iter()
                .filter(|c| **c == CLS_UNKNOWN || **c == CLS_NODATA)
                .count() as f64
                / cells.max(1) as f64
        }
        None => 0.0,
    };
    let best = &loaded[0];
    let cand = vrec.candidates.iter().find(|c| c.pano_id == best.pano_id);
    let quality = cand
        .and_then(|c| c.gate(GateName::Quality))
        .map(|g| g.value)
        .or_else(|| ctx.geo.metas.get(&best.pano_id).map(|m| m.quality))
        .unwrap_or(1.0);
    let blur_factor = cand
        .and_then(|c| c.gate(GateName::BlurRel))
        .map(|g| g.value.clamp(0.35, 1.0))
        .unwrap_or(1.0);
    let cam = &align.cameras[&best.pano_id];
    let refs: Vec<Refinement> = loaded.iter().map(|v| v.refinement.clone()).collect();
    let factors = confidence::factors_for(
        Some(cam.pose_source),
        cam.reg.as_ref(),
        Some(&fit),
        &dec,
        &refs,
        fused.agreement_m,
        loaded.len(),
        unknown_share,
        quality.max(0.05),
        blur_factor,
        Some(wall.height_source.as_str()),
    );
    let (conf, tier) = confidence::score(&factors, params);
    let mut flags = merged(&dec.flags, &fused.flags);
    if fused.mode == fuse::FuseMode::Single {
        flags.push("SINGLE_VIEW".to_string());
    }
    for code in confidence::reason_codes(&vrec.candidates, Some(&dec), &refs, cam.reg.as_ref(), &[])
    {
        if !flags.contains(&code) {
            flags.push(code);
        }
    }
    flags.sort();

    if let Some(debug) = cfg.debug.as_ref().filter(|d| d.wants(&wall.key)) {
        dump_wall(
            debug, wall, vrec, &loaded, &view_tex, &fused, &structure, &dec, tier, conf,
        );
    }

    let observed: Vec<bool> = structure
        .openings
        .as_ref()
        .map(|op| op.cls.iter().map(|c| *c != CLS_NODATA).collect())
        .unwrap_or_else(|| vec![true; cells]);
    WallProduct {
        wall_key: wall.key.clone(),
        building_key: wall.building_key.clone(),
        node_a: wall.node_a,
        node_b: wall.node_b,
        piece: wall.piece,
        n_pieces: wall.n_pieces,
        edges: wall.edges.clone(),
        col0_m: s_l,
        cols,
        rows,
        rgb: structure.rgb.clone(),
        cls: structure.cls.clone(),
        observed,
        bands: row_band_colours(&structure),
        tex: Some(rgba(&fused.rgb, &fused.valid)),
        tier,
        confidence: conf,
        height_used_m: dec.h_used,
        unknown_share,
        views: loaded.iter().map(|v| v.pano_id.clone()).collect(),
        flags,
    }
}

/// The wall product of a wall without a usable view: tier C when a line of
/// sight clean candidate existed and only the image gates turned it down, tier
/// D when nothing could ever have seen it.
fn no_view_product(wall: &Wall, vrec: &WallViews) -> WallProduct {
    // The align stage re-marked reachability from the registered centres, so
    // its answer is the one that counts, not the wall's optimistic default.
    let reachable = vrec.reachable;
    let reason = if !reachable {
        if vrec.unreachable_reason.is_empty() {
            "unreachable"
        } else {
            vrec.unreachable_reason.as_str()
        }
    } else if vrec.n_los_clean > 0 {
        "no_view_passed_image_gates"
    } else {
        "no_line_of_sight"
    };
    let tier = if reachable && vrec.n_los_clean > 0 {
        Tier::C
    } else {
        Tier::D
    };
    let flags =
        confidence::reason_codes(&vrec.candidates, None, &[], None, &[reason.to_uppercase()]);
    WallProduct {
        wall_key: wall.key.clone(),
        building_key: wall.building_key.clone(),
        node_a: wall.node_a,
        node_b: wall.node_b,
        piece: wall.piece,
        n_pieces: wall.n_pieces,
        edges: wall.edges.clone(),
        col0_m: 0.0,
        cols: 0,
        rows: 0,
        rgb: Vec::new(),
        cls: Vec::new(),
        observed: Vec::new(),
        bands: Vec::new(),
        tex: None,
        tier,
        confidence: 0.0,
        height_used_m: 0.0,
        unknown_share: 1.0,
        views: Vec::new(),
        flags,
    }
}

/// Per row, the colour the band pass painted its wall cells, which is what the
/// consumer's colour-only mode reads.
fn row_band_colours(st: &Structure) -> Vec<[u8; 3]> {
    (0..st.rows)
        .map(|r| {
            st.bands
                .iter()
                .find(|b| b.r0 <= r && r <= b.r1)
                .and_then(|b| b.rgb)
                .unwrap_or([128, 128, 128])
        })
        .collect()
}

/// The fused texture as the RGBA the export carries: colour with the valid mask
/// in alpha.
fn rgba(rgb: &RgbImage, valid: &[bool]) -> RgbaImage {
    let mut out = RgbaImage::new(rgb.width(), rgb.height());
    for (i, pixel) in out.pixels_mut().enumerate() {
        let p = rgb.as_raw();
        *pixel = image::Rgba([
            p[3 * i],
            p[3 * i + 1],
            p[3 * i + 2],
            if valid.get(i).copied().unwrap_or(false) {
                255
            } else {
                0
            },
        ]);
    }
    out
}

/// `a` and `b` without duplicates, in `a`'s order then `b`'s.
fn merged(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = a.to_vec();
    for s in b {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    out
}

/// The node ids of a wall in node order, the identity the cache keys on.
///
/// The same list `cache::wall_node_ids` builds from a finished product, only
/// from the wall itself so the cache can be consulted before the work is done.
pub fn wall_node_ids(wall: &Wall) -> Vec<i64> {
    if wall.edges.is_empty() {
        return vec![wall.node_a, wall.node_b];
    }
    let mut ids = vec![wall.edges[0].node_a];
    for e in &wall.edges {
        ids.push(e.node_b);
    }
    ids
}

/// The texture stage over every wall of the run, and how many of them the cache
/// answered.
///
/// Walls are independent after registration, which is the property the whole
/// port is arranged around, so this is a plain parallel map. A wall already in
/// the cache is not built again: the lookup is by the wall's OSM node ids and
/// the params digest, so it survives a different bbox but not a threshold
/// change. The lookup runs even for a wall this run found no view for, which is
/// the point of keying on node ids: a bbox drawn a street further along cuts
/// off the camera that saw a wall, and the facade it already has is better than
/// the tier D it would get now.
///
/// A wall that produced no grid is never stored. Its view list is a fact about
/// this bbox's imagery rather than about the wall, so caching it would make the
/// first small bbox poison every later one.
pub fn stage_texture(
    cfg: &PipelineConfig,
    geo: &Geometry,
    align: &Align,
    images: &BTreeMap<String, PathBuf>,
    originals: &BTreeMap<String, PathBuf>,
) -> (Vec<WallProduct>, usize, usize) {
    let ctx = WallContext {
        cfg,
        geo,
        align,
        images,
        originals,
    };
    let hits = AtomicUsize::new(0);
    let misses = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let total = geo.walls.len();
    let empty = WallViews::default();
    let build = || -> Vec<WallProduct> {
        geo.walls
            .par_iter()
            .map(|wall| {
                let vrec = align.views.get(&wall.key).unwrap_or(&empty);
                if cfg.cancelled() {
                    return no_view_product(wall, vrec);
                }
                let key = cache::wall_cache_key(&wall_node_ids(wall), wall.piece);
                // A cached verdict of "no facade" is not a hit here: this run
                // has its own view list and rebuilding a wall it has views for
                // costs one wall, where believing a stale blank costs a facade.
                // `run_from_cache` is where a verdict is reused, under the rule
                // that says when it is a fact about the wall.
                let cached = cache::load_wall(&cfg.facade_cache, &key)
                    .map(|c| c.product)
                    .filter(|p| p.cols > 0);
                let product = match cached {
                    Some(mut cached) => {
                        hits.fetch_add(1, Ordering::Relaxed);
                        // The cache is keyed by node ids, so the run that built
                        // the wall can have named it by a different ring index;
                        // this run's key is the one the export writes.
                        cached.wall_key = wall.key.clone();
                        cached.building_key = wall.building_key.clone();
                        cached
                    }
                    None if vrec.views.is_empty() => no_view_product(wall, vrec),
                    None => {
                        misses.fetch_add(1, Ordering::Relaxed);
                        let built = texture_wall(&ctx, wall);
                        if built.cols > 0 {
                            let reach = search_reach_m(cfg, geo.bbox_xy, wall);
                            if let Err(e) = cache::store_wall(&cfg.facade_cache, &built, reach) {
                                eprintln!("Note: facade cache write for {}: {e}", wall.key);
                            }
                        }
                        built
                    }
                };
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n.is_multiple_of((total / 20).max(1)) || n == total {
                    emit_gui_progress_update(
                        MESSAGE_ONLY,
                        &format!("Mapillary facades: building {n}/{total}..."),
                    );
                }
                product
            })
            .collect()
    };
    let products = in_pool(cfg.threads, build);
    (
        products,
        hits.load(Ordering::Relaxed),
        misses.load(Ordering::Relaxed),
    )
}

/// Runs `body` on the crate's own pool unless a narrower width was asked for.
///
/// `floodfill_cache::configure_rayon_thread_pool` already sized the global pool
/// at 90 per cent of the cores for the whole process, and every other CPU stage
/// runs on it; a second pool of the same width would only oversubscribe.
fn in_pool<T: Send>(threads: usize, body: impl FnOnce() -> T + Send) -> T {
    if threads == 0 {
        return body();
    }
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.clamp(1, 64))
        .build()
    {
        Ok(pool) => pool.install(body),
        Err(_) => body(),
    }
}

// --------------------------------------------------------------------------- the export

/// Confidence weighted median OkLab of the observed wall blocks of a building.
///
/// The band from 2 m to a metre below the roof, with vegetation and sky
/// coloured cells excluded, which is the part of a facade that is actually the
/// building's colour rather than its shopfront or its sky reflection.
fn building_colour(grids: &[(&WallProduct, f64)]) -> (Option<[u8; 3]>, f64) {
    let mut labs: Vec<[f64; 3]> = Vec::new();
    let mut weights: Vec<f64> = Vec::new();
    let mut confs: Vec<f64> = Vec::new();
    for (product, conf) in grids {
        let (rows, cols) = (product.rows as usize, product.cols as usize);
        if rows == 0 || cols == 0 {
            continue;
        }
        let h_used = if product.height_used_m > 0.0 {
            product.height_used_m
        } else {
            rows as f64
        };
        let in_band = |r: usize| {
            let h_centre = rows as f64 - r as f64 - 0.5;
            h_centre >= 2.0 && h_centre <= h_used - 1.0
        };
        let any_band = (0..rows).any(in_band);
        let mut picked: Vec<[f64; 3]> = Vec::new();
        for r in 0..rows {
            if any_band && !in_band(r) {
                continue;
            }
            for c in 0..cols {
                let i = r * cols + c;
                if product.cls[i] != super::openings::CLS_WALL || !product.observed[i] {
                    continue;
                }
                let lab = imgops::srgb_to_oklab(product.rgb[i]);
                if lab[1] < GREEN_A || (lab[2] < SKY_B && lab[0] > 0.6) {
                    continue;
                }
                picked.push(lab);
            }
        }
        if picked.is_empty() {
            continue;
        }
        weights.extend(std::iter::repeat_n(conf.max(1e-3), picked.len()));
        labs.extend(picked);
        confs.push(*conf);
    }
    if labs.is_empty() {
        return (None, 0.0);
    }
    let mut med = [0.0f64; 3];
    for (c, out) in med.iter_mut().enumerate() {
        let values: Vec<f64> = labs.iter().map(|l| l[c]).collect();
        *out = lower_weighted_median(&values, &weights);
    }
    let rgb = imgops::oklab_to_rgb8(med);
    let mut deviations: Vec<f64> = labs
        .iter()
        .map(|l| {
            let p = imgops::oklab_to_rgb8(*l);
            (0..3)
                .map(|k| (f64::from(p[k]) - f64::from(rgb[k])).abs())
                .sum::<f64>()
                / 3.0
        })
        .collect();
    let mad = imgops::median_in_place(&mut deviations);
    let best = confs.iter().copied().fold(0.0f64, f64::max);
    let conf = (1.0 - mad / COLOUR_MAD_SCALE).clamp(0.0, 1.0) * best.clamp(0.0, 1.0);
    (Some(rgb), conf)
}

/// The weighted median as `blocks.py` takes it: the first sample whose
/// cumulative weight reaches half the total, with no interpolation.
fn lower_weighted_median(values: &[f64], weights: &[f64]) -> f64 {
    let mut idx: Vec<usize> = (0..values.len()).collect();
    idx.sort_by(|a, b| values[*a].total_cmp(&values[*b]));
    let total: f64 = weights.iter().sum();
    let mut cum = 0.0;
    for i in &idx {
        cum += weights[*i];
        if cum >= 0.5 * total {
            return values[*i];
        }
    }
    idx.last().map(|i| values[*i]).unwrap_or(f64::NAN)
}

/// Per original OSM edge, its `s` interval on this piece and the block columns
/// it covers.
fn edge_intervals(product: &WallProduct) -> Vec<Value> {
    let cols = product.cols as i64;
    let fallback = [WallEdge {
        edge_idx: usize::MAX,
        node_a: product.node_a,
        node_b: product.node_b,
        s0: 0.0,
        s1: f64::from(product.cols),
    }];
    let edges: &[WallEdge] = if product.edges.is_empty() {
        &fallback
    } else {
        &product.edges
    };
    edges
        .iter()
        .map(|e| {
            let c0 = e.s0 - product.col0_m;
            let c1 = e.s1 - product.col0_m;
            json!({
                "edge_idx": if e.edge_idx == usize::MAX { -1 } else { e.edge_idx as i64 },
                "node_a": e.node_a,
                "node_b": e.node_b,
                "s0": e.s0,
                "s1": e.s1,
                "col0": (c0 + 1e-9).floor().clamp(0.0, cols as f64) as i64,
                "col1": (c1 - 1e-9).ceil().clamp(0.0, cols as f64) as i64,
            })
        })
        .collect()
}

/// The wall record `facades::parse_wall` reads, in the field names
/// `export_arnis.py` writes.
fn wall_record(product: &WallProduct, wall: &Wall, frame: &Frame, osm_id: i64) -> Value {
    let a = frame.to_lonlat(wall.a);
    let b = frame.to_lonlat(wall.b);
    let exported = matches!(product.tier, Tier::A | Tier::B) && product.cols > 0;
    json!({
        "key": product.wall_key,
        "building_key": product.building_key,
        "osm_id": osm_id,
        "idx": wall.idx,
        "piece": wall.piece,
        "n_pieces": wall.n_pieces,
        "s_offset": wall.s_offset,
        "node_a": product.node_a,
        "node_b": product.node_b,
        "node_ids": wall_node_ids(wall),
        "a_lonlat": [a[0], a[1]],
        "b_lonlat": [b[0], b[1]],
        "a_enu": wall.a,
        "b_enu": wall.b,
        "length_m": wall.length,
        "cols": if exported { Value::from(product.cols) } else { Value::Null },
        "rows": if exported { Value::from(product.rows) } else { Value::Null },
        "edges": edge_intervals(product),
        "height_osm_m": wall.height_osm,
        "height_osm_source": wall.height_source.as_str(),
        "height_used_m": if exported { Value::from(product.height_used_m) } else { Value::Null },
        // Where the height came from is in `flags` (ROOF_SKY, ROOF_EDGE,
        // ROOF_OSM), which is what the cache carries, so it is not repeated as
        // a field that would only ever be filled in from the flags.

        "extent": if exported {
            json!({ "s_l": product.col0_m, "s_r": product.col0_m + f64::from(product.cols) })
        } else {
            Value::Null
        },
        "views": product.views,
        "confidence": product.confidence,
        "tier": product.tier.as_str(),
        "flags": product.flags,
        "unknown_share": product.unknown_share,
        "reachable": wall.reachable,
        "unreachable_reason": wall.unreachable_reason,
        "png": if exported { Value::from(format!("{}.png", product.wall_key)) } else { Value::Null },
        "cls_png": if exported { Value::from(format!("{}_cls.png", product.wall_key)) } else { Value::Null },
        "tex": if exported && product.tex.is_some() {
            Value::from(format!("{}_tex.png", product.wall_key))
        } else {
            Value::Null
        },
    })
}

/// Writes a finished run into the cache in the export format
/// `facades::install` reads: per building one JSON, per exported wall the 1 px
/// per metre block PNG with the class in alpha, its class PNG and the 8 px per
/// metre texture.
pub fn write_export(result: &PipelineResult, dir: &Path) -> Result<(), String> {
    // `create_dir` and not `create_dir_all`: the export is one run's, and the
    // directory is how it says so. Claiming it is what makes two generations at
    // once impossible to mix, whatever the file system does underneath, and it
    // is why nothing here has to clear anything first. It also means a caller
    // that hands the same directory to two runs is told, rather than quietly
    // getting one run's images under another run's manifest.
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    std::fs::create_dir(dir).map_err(|e| format!("claim {}: {e}", dir.display()))?;

    let frame = result.frame;
    let walls: BTreeMap<&str, &Wall> = result.walls.iter().map(|w| (w.key.as_str(), w)).collect();
    let mut by_building: BTreeMap<&str, Vec<&WallProduct>> = BTreeMap::new();
    for p in &result.products {
        by_building
            .entry(p.building_key.as_str())
            .or_default()
            .push(p);
    }

    let mut n_buildings = 0usize;
    let mut n_walls = 0usize;
    for building in &result.buildings {
        let Some(products) = by_building.get(building.key.as_str()) else {
            continue;
        };
        let mut ordered: Vec<&&WallProduct> = products.iter().collect();
        ordered.sort_by_key(|p| {
            walls
                .get(p.wall_key.as_str())
                .map(|w| (w.idx, w.piece))
                .unwrap_or((usize::MAX, 0))
        });
        let colour_input: Vec<(&WallProduct, f64)> = ordered
            .iter()
            .filter(|p| matches!(p.tier, Tier::A | Tier::B | Tier::C) && p.cols > 0)
            .map(|p| (**p, p.confidence))
            .collect();
        let (colour, colour_conf) = building_colour(&colour_input);

        let mut records = Vec::new();
        let mut exported = 0usize;
        for product in &ordered {
            let Some(wall) = walls.get(product.wall_key.as_str()) else {
                continue;
            };
            records.push(wall_record(product, wall, &frame, building.osm_id));
            if !matches!(product.tier, Tier::A | Tier::B) || product.cols == 0 {
                continue;
            }
            write_wall_images(dir, product)?;
            exported += 1;
        }
        if exported == 0 {
            continue;
        }
        let tiers: Map<String, Value> = ["A", "B", "C", "D"]
            .iter()
            .map(|t| {
                (
                    (*t).to_string(),
                    Value::from(ordered.iter().filter(|p| p.tier.as_str() == *t).count()),
                )
            })
            .collect();
        let record = json!({
            "key": building.key,
            "osm_id": building.osm_id,
            "kind": building.kind.as_str(),
            "way_id": (building.kind == super::types::OsmKind::Way).then_some(building.osm_id),
            "relation_id": (building.kind == super::types::OsmKind::Relation).then_some(building.osm_id),
            "member_ways": building.member_ways,
            "height_osm_m": building.height_osm,
            "height_source": building.height_source.as_str(),
            "min_height_m": building.min_height,
            "target": building.target,
            // The same handful the Python writes: a facade record is not the
            // place for an addr:* dump of every building in the box.
            "tags": building.tags.iter()
                .filter(|(k, _)| matches!(k.as_str(), "building" | "name" | "building:levels"
                    | "height" | "roof:levels" | "addr:street" | "addr:housenumber"))
                .collect::<BTreeMap<_, _>>(),
            "frame": {
                "lon0": frame.lon0,
                "lat0": frame.lat0,
                "radius_m": 6_371_000.0,
                "note": "ENU metres about lon0/lat0; Arnis world plan = [x, -y]",
            },
            "block_note": BLOCK_NOTE,
            "building_colour": colour.map(|c| json!({
                "rgb": [c[0], c[1], c[2]],
                "hex": format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
                "confidence": colour_conf,
            })),
            "ring_lonlat": building
                .ring
                .iter()
                .map(|p| { let ll = frame.to_lonlat(*p); [ll[0], ll[1]] })
                .collect::<Vec<_>>(),
            "node_ids": building.node_ids,
            "tiers": tiers,
            "n_walls": records.len(),
            "walls": records,
            "exported_walls": exported,
        });
        let path = dir.join(format!("{}.json", building.key));
        std::fs::write(
            &path,
            serde_json::to_vec(&record).map_err(|e| format!("{}: {e}", path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        n_buildings += 1;
        n_walls += exported;
    }

    let manifest = json!({
        "generator": "src/mapillary/pipeline.rs",
        "min_tier": "B",
        "buildings": n_buildings,
        "walls": n_walls,
        "block_size_m": 1.0,
        "cells": "floor bands + completed window lattice (bands.rs)",
        "layout": "<building_key>.json + <wall_key>.png (RGB colour, alpha class) \
                   + <wall_key>_cls.png + <wall_key>_tex.png (8 px/m)",
        // The attribution travels with the export, so a folder handed to
        // `--mapillary-facades-dir` later can still name the photographers.
        // The per wall `views` carry the ids either way; only the names would
        // otherwise be lost with the run that fetched them.
        "licence": "Mapillary imagery is CC BY-SA 4.0",
        "credits": result.credits.iter().map(|c| json!({
            "id": c.pano_id,
            "title": c.title,
            "creator": c.creator,
            "creator_url": c.creator_url,
            "image_url": c.image_url,
        })).collect::<Vec<_>>(),
    });
    let path = dir.join("manifest.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// A directory of this run's own, under `facades/<digest>/exports/`.
///
/// The name carries the bbox so a human can tell which box an export is of, and
/// a nonce so two runs of the same box, in one process or two, cannot collide.
fn mint_export_dir(cfg: &PipelineConfig) -> PathBuf {
    use std::hash::Hasher;
    let b = cfg.fetch.bbox;
    let mut h = fnv::FnvHasher::default();
    for v in [b.min_lat, b.min_lon, b.max_lat, b.max_lon] {
        h.write(&(v * 1e7).round().to_bits().to_le_bytes());
    }
    let area = format!("{:016x}", h.finish());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = EXPORT_COUNTER.fetch_add(1, Ordering::Relaxed);
    cfg.exports_dir()
        .join(format!("{area}-{now:x}-{:x}-{n:x}", std::process::id()))
}

static EXPORT_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// How long an export directory is left alone before it is swept.
///
/// A run's export is read by `facades::install` while the world is being built,
/// which is minutes, and nothing reads one afterwards. An hour is far longer
/// than any generation and short enough that the exports of a day's work do not
/// pile up; a directory somebody still has a file open in cannot be removed on
/// Windows anyway, and the failure is ignored. The thirty day sweep in
/// `elevation::cache` is the backstop for whatever this misses.
const EXPORT_RETENTION: std::time::Duration = std::time::Duration::from_secs(3600);

/// How many areas keep an export past [`EXPORT_RETENTION`] for the preview.
///
/// A count and not a size, because the count is what has to be bounded: without
/// it every box the user ever drew would keep an export for thirty days. What
/// one costs varies with how much facade the box holds, and the small end is
/// measured: the Munich test box exports 111 walls in 4.8 MB, beside 4.8 MB of
/// per wall products and 600 MB of imagery for the same area. The largest box
/// anyone waits an hour of pipeline for is maybe fifteen times that, so eight
/// of those is a few hundred megabytes, and the thirty day sweep and the clear
/// cache button both reach it.
const EXPORT_KEEP_AREAS: usize = 8;

/// Removes export directories older than `retention`, `keep` and the newest of
/// each area excepted. Best effort: a failure here costs disk, not a run.
///
/// The newest of each area survives because it is what the 3D preview shows
/// (`facades::preview_walls_from_cache`). Without that rule an area precomputed
/// an hour ago is still in the per wall cache, so the next generation over it
/// is instant, but the preview goes blank the moment any other run sweeps, and
/// the user is told nothing was computed when it was. One export per box
/// precomputed is a few megabytes and clearing the cache takes it.
fn sweep_exports(exports: &Path, keep: &Path, retention: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(exports) else {
        return;
    };
    let now = std::time::SystemTime::now();
    // The area is the leading field of the directory name minted by
    // `mint_export_dir`, so two runs of one box group without reading anything.
    let mut newest: HashMap<String, (std::time::SystemTime, PathBuf)> = HashMap::new();
    let mut aged: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let area = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').next())
            .unwrap_or_default()
            .to_string();
        let best = newest
            .entry(area)
            .or_insert_with(|| (modified, path.clone()));
        // The path breaks a tie, the same way the preview's own ordering does,
        // so the pair never disagree about which of two is the newer.
        if (modified, &path) > (best.0, &best.1) {
            aged.push(std::mem::replace(best, (modified, path)).1);
        } else if path != best.1 {
            aged.push(path);
        }
    }
    // This run's own area keeps `keep`, which is younger than anything here, so
    // the survivor of that group is swept like the rest once it ages out.
    let keep_area = keep
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('-').next())
        .unwrap_or_default();
    if let Some((_, path)) = newest.remove(keep_area) {
        aged.push(path);
    }
    // And only the most recent areas are spared, or a year of generating would
    // keep an export of every box the user ever drew. Beyond this the per wall
    // cache still holds the walls, so the area is still built instantly; only
    // the preview has to be given a run to read, which is one press of
    // Precompute or one generation.
    let mut survivors: Vec<(std::time::SystemTime, PathBuf)> = newest.into_values().collect();
    survivors.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    aged.extend(
        survivors
            .into_iter()
            .skip(EXPORT_KEEP_AREAS)
            .map(|(_, p)| p),
    );

    for path in aged {
        let old = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > retention);
        if old {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

const BLOCK_NOTE: &str = "Blocks are 1 m x 1 m at Arnis scale 1 (worlds with scale != 1 must \
resample). Columns run from node_a (col 0, s = s_l) towards node_b; if the Arnis edge runs \
node_b -> node_a, flip the columns. Rows run from the top (row 0) to the wall base (last row). \
Run frame is ENU metres about frame.lon0/lat0 (x east, y north); Arnis world plan = [x, -y].";

/// The three PNGs of one exported wall.
fn write_wall_images(dir: &Path, product: &WallProduct) -> Result<(), String> {
    let mut blocks = RgbaImage::new(product.cols, product.rows);
    for (i, pixel) in blocks.pixels_mut().enumerate() {
        let [r, g, b] = product.rgb[i];
        *pixel = image::Rgba([r, g, b, product.cls[i]]);
    }
    write_png(&dir.join(format!("{}.png", product.wall_key)), &blocks)?;

    let mut classes = image::GrayImage::new(product.cols, product.rows);
    for (i, pixel) in classes.pixels_mut().enumerate() {
        *pixel = image::Luma([product.cls[i]]);
    }
    write_png(&dir.join(format!("{}_cls.png", product.wall_key)), &classes)?;

    if let Some(tex) = &product.tex {
        write_png(&dir.join(format!("{}_tex.png", product.wall_key)), tex)?;
    }
    Ok(())
}

fn write_png<P, C>(path: &Path, img: &image::ImageBuffer<P, C>) -> Result<(), String>
where
    P: image::Pixel<Subpixel = u8> + image::PixelWithColorType,
    C: std::ops::Deref<Target = [u8]>,
{
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

// --------------------------------------------------------------------------- the debug dump

/// Everything one wall was made of, written where the Python review figures can
/// pick it up: the loose crops and their occlusion bits, the per view textures,
/// the fused texture, the block grid and the gate table.
#[allow(clippy::too_many_arguments)]
fn dump_wall(
    debug: &DebugDump,
    wall: &Wall,
    vrec: &WallViews,
    loaded: &[LoadedView],
    view_tex: &[ViewTexture],
    fused: &fuse::FusedTexture,
    structure: &Structure,
    dec: &WallDecision,
    tier: Tier,
    confidence: f64,
) {
    let dir = &debug.dir;
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("Note: facade debug dump {}: {e}", dir.display());
        return;
    }
    let key = &wall.key;
    for v in loaded {
        let base = format!("{key}__{}", v.pano_id);
        let _ = write_png(&dir.join(format!("{base}_crop_rgb.png")), &v.crop.rgb);
        let mut occl = image::GrayImage::new(v.crop.rgb.width(), v.crop.rgb.height());
        for (i, pixel) in occl.pixels_mut().enumerate() {
            *pixel = image::Luma([v.crop.occl.get(i).copied().unwrap_or(0)]);
        }
        let _ = write_png(&dir.join(format!("{base}_crop_occl.png")), &occl);
    }
    for v in view_tex {
        let _ = write_png(
            &dir.join(format!("{key}__{}_tex.png", v.pano_id)),
            &rgba(&v.rgb, &v.valid),
        );
    }
    let _ = write_png(
        &dir.join(format!("{key}_tex.png")),
        &rgba(&fused.rgb, &fused.valid),
    );
    let mut blocks = RgbaImage::new(structure.cols as u32, structure.rows as u32);
    for (i, pixel) in blocks.pixels_mut().enumerate() {
        let [r, g, b] = structure.rgb[i];
        *pixel = image::Rgba([r, g, b, structure.cls[i]]);
    }
    let _ = write_png(&dir.join(format!("{key}_blocks.png")), &blocks);

    let mut gates = String::from("pano\tgate\tvalue\tpassed\trejected_reason\n");
    for c in &vrec.candidates {
        for g in &c.gates {
            gates.push_str(&format!(
                "{}\t{}\t{:.6}\t{}\t{}\n",
                c.pano_id,
                g.name.as_str(),
                g.value,
                g.passed,
                c.rejected_reason.as_deref().unwrap_or("")
            ));
        }
    }
    let _ = std::fs::write(dir.join(format!("{key}_gates.tsv")), gates);

    let record = json!({
        "key": key,
        "tier": tier.as_str(),
        "confidence": confidence,
        "cols": structure.cols,
        "rows": structure.rows,
        "extent": { "s_l": dec.s_l, "s_r": dec.s_r },
        "height_used_m": dec.h_used,
        "height_source": dec.height_source,
        "phase": dec.phase,
        "bimodality": dec.bimodality,
        "flags": dec.flags,
        "fuse": {
            "mode": fused.mode.as_str(),
            "agreement_m": fused.agreement_m,
            "hole_fraction": fused.hole_fraction,
            "shifts_m": fused.shifts,
            "flags": fused.flags,
        },
        "views": loaded.iter().map(|v| json!({
            "pano": v.pano_id,
            "score": v.score,
            "dist_m": v.dist_m,
            "ds_m": v.ds_m,
            "dz_m": v.dz,
            "z_base": v.z_base,
            "h_cloud": v.h_cloud,
            "lean_flag": v.refinement.lean_flag,
            "plane_flag": v.refinement.plane_flag,
            "roof_flag": v.refinement.roof_flag,
            "ground_flag": v.refinement.ground_flag,
            "h_sky": v.refinement.h_sky,
        })).collect::<Vec<_>>(),
        "bands": structure.bands.iter().map(|b| json!({
            "rows": [b.r0, b.r1],
            "rgb": b.rgb,
            "window_rgb": b.window_rgb,
        })).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(
        dir.join(format!("{key}.json")),
        serde_json::to_vec_pretty(&record).unwrap_or_default(),
    );
}

// --------------------------------------------------------------------------- the driver

/// Runs every stage for one world bbox, from the network.
pub fn run(cfg: &PipelineConfig) -> Result<PipelineResult, String> {
    credits::reset();
    let t0 = Instant::now();
    // Nothing has ever been built with these tunables, so a cache-only run has
    // nothing to look for and there is no reason to ask Overpass either. One
    // arm with a guard rather than two nested `if`s, which edition 2024's
    // `collapsible_if` would want written as a let chain this edition has not
    // got.
    match &cfg.cache_only {
        Some(why) if !cfg.facade_cache.exists() => return Err(why.clone()),
        _ => {}
    }
    // The buildings come first, and not because Overpass is the interesting
    // half: they are what says which walls this run needs, and if the cache
    // already holds every one of them there is no reason to search Mapillary
    // for imagery, let alone download it.
    emit_gui_progress_update(
        MESSAGE_ONLY,
        "Mapillary facades: reading OpenStreetMap buildings...",
    );
    let osm = fetch::fetch_osm(&cfg.fetch)?;
    run_from_osm(cfg, osm, t0)
}

/// The rest of [`run`] once Overpass has answered: the cache first, then the
/// cold path.
///
/// Split out from `run` so the cache-only rule can be tested against a fixture
/// Overpass answer, which is the only way to reach it without a network. `t0` is
/// the whole run's clock and belongs to the caller, because the Overpass query
/// is part of what `stats.fetch_s` reports.
fn run_from_osm(cfg: &PipelineConfig, osm: Value, t0: Instant) -> Result<PipelineResult, String> {
    if let Some(mut result) = run_from_cache(cfg, &osm)? {
        result.stats.fetch_s = t0.elapsed().as_secs_f64();
        result.stats.total_s = t0.elapsed().as_secs_f64();
        return Ok(result);
    }
    // The cache could not answer, and this caller may not pay for the cold path.
    // Everything past here is the part that takes hours on a large box, and the
    // caller that sets `cache_only` is one whose world is waiting on the answer.
    if let Some(why) = &cfg.cache_only {
        return Err(why.clone());
    }
    // The imagery search is the first thing that costs the network, and on a
    // cold area everything after it runs for minutes without a stop, so this is
    // the last cheap place a cancelled run can be turned back.
    cfg.check()?;
    emit_gui_progress_update(MESSAGE_ONLY, "Mapillary facades: searching coverage...");
    let mut fetched = fetch::fetch_metadata(&cfg.fetch)?;
    fetched.osm = osm;
    let fetch_s = t0.elapsed().as_secs_f64();
    let mut result = run_with(cfg, &fetched)?;
    result.stats.fetch_s = fetch_s;
    result.stats.total_s = t0.elapsed().as_secs_f64();
    Ok(result)
}

/// The frame, the footprints and the walls, which is everything the OSM half of
/// the fetch decides on its own.
fn footprints_and_walls(cfg: &PipelineConfig, osm: &Value) -> (Frame, Vec<Building>, Vec<Wall>) {
    let frame = geometry::build_frame(cfg.fetch.bbox);
    let buildings = geometry::parse_overpass(osm, &frame, &cfg.params, Some(cfg.fetch.bbox));
    let walls = geometry::walls_from_buildings(&buildings, &cfg.params);
    (frame, buildings, walls)
}

/// The run bbox in the run frame, which every wall is measured against.
fn bbox_in_frame(cfg: &PipelineConfig, frame: &Frame) -> [f64; 4] {
    let lo = frame.to_enu(cfg.fetch.bbox.min_lon, cfg.fetch.bbox.min_lat);
    let hi = frame.to_enu(cfg.fetch.bbox.max_lon, cfg.fetch.bbox.max_lat);
    [lo[0], lo[1], hi[0], hi[1]]
}

/// How much of a wall's surroundings this run's imagery search covered, in
/// metres, capped at the furthest a camera may be from a wall.
///
/// This is what turns "no facade" from a fact about the box into a fact about
/// the wall. The search covers the box grown by `fetch.pano_margin_m`, and
/// `params.far_dist_m` is the furthest a camera may be from a wall and still be
/// a candidate (`visibility::mark_reachable`), so a wall `far_dist_m` inside
/// the searched box has been judged against every photograph that could ever
/// see it and no later box can find it one more. With the defaults the two are
/// both 45 m, so **every wall inside the world box reads the cap**, which is
/// every wall a world can contain. A wall that reads less than the cap is one
/// this box only half looked at, and it is only ever an occluder here.
fn search_reach_m(cfg: &PipelineConfig, bbox_xy: [f64; 4], wall: &Wall) -> f64 {
    let pad = cfg.fetch.pano_margin_m;
    let [x0, y0, x1, y1] = bbox_xy;
    let reach = |p: [f64; 2]| {
        (p[0] - (x0 - pad))
            .min((x1 + pad) - p[0])
            .min(p[1] - (y0 - pad))
            .min((y1 + pad) - p[1])
    };
    reach(wall.a)
        .min(reach(wall.b))
        .clamp(0.0, cfg.params.far_dist_m)
}

/// How much further than the recorded search this run may reach round a wall
/// and still believe the recorded verdict.
///
/// Without it the reuse rule has a hard edge somewhere on the ground, and a box
/// the user nudged puts a handful of walls on the other side of it and redoes an
/// area's align work for them. A metre is the width of the strip of new ground
/// such a nudge adds, and the only cameras in it are ones the distance gate
/// meets at the very limit of `far_dist_m`, where a view is being rejected for
/// range anyway. A box that genuinely reaches further round a wall reaches it by
/// tens of metres, not by one.
const SEARCH_REACH_SLACK_M: f64 = 1.0;

/// Files what this run proved about the walls that carry no facade.
///
/// A wall that came back with no blocks cost the whole align stage to decide,
/// and that answer is per wall like every other: on the Munich box it is 912 of
/// 1030 walls, so without it a later generation over the same buildings has an
/// answer for a tenth of them and redoes the rest.
///
/// It used to be filed per bbox, under a hash of the four corners, which meant
/// a box moved by 22 cm found no record at all and repeated an area's align
/// work for walls that were every one of them already cached. What replaces the
/// bbox is [`search_reach_m`], which is a property of the wall and the search
/// rather than of the box.
fn store_blank_verdicts<'a>(
    cfg: &PipelineConfig,
    bbox_xy: [f64; 4],
    walls: impl Iterator<Item = (&'a Wall, &'a WallProduct)>,
) {
    for (wall, product) in walls {
        if product.cols != 0 {
            continue;
        }
        let reach = search_reach_m(cfg, bbox_xy, wall);
        if let Err(e) = cache::store_wall(&cfg.facade_cache, product, reach) {
            eprintln!("Note: facade cache verdict for {}: {e}", wall.key);
        }
    }
}

/// The whole run from the cache, or `None` when a single wall is missing.
///
/// This is the path a second generation over an area already built takes: no
/// imagery search, no downloads, no registration and no image decoding at all,
/// only the export written again from what is already on disk. It is taken
/// whatever bbox the run asks for, because every question it asks is per wall.
///
/// A wall answers in one of two ways:
///
/// * it has a cached product, which is a fact about the wall and good for any
///   box;
/// * it has a cached "no facade" verdict from a run that had searched as much
///   of the wall's surroundings as this one would (see [`search_reach_m`]).
///   Every wall inside the world box reads the cap, so for those this says the
///   verdict was proved against every photograph that could ever see the wall;
///   for the occluders outside it, it says only that nobody looked any harder.
///
/// Anything else and the run does the work.
fn run_from_cache(cfg: &PipelineConfig, osm: &Value) -> Result<Option<PipelineResult>, String> {
    // Nothing has ever been built with these tunables, so there is no point
    // parsing the footprints twice to find that out one wall at a time.
    if !cfg.facade_cache.exists() {
        return Ok(None);
    }
    let (frame, buildings, mut walls) = footprints_and_walls(cfg, osm);
    if buildings.is_empty() {
        return Ok(None);
    }
    let bbox_xy = bbox_in_frame(cfg, &frame);
    let found: Vec<Option<WallProduct>> = walls
        .par_iter()
        .map(|wall| {
            let key = cache::wall_cache_key(&wall_node_ids(wall), wall.piece);
            let cached = cache::load_wall(&cfg.facade_cache, &key)?;
            let good = cached.product.cols > 0
                || cached.reach_m + SEARCH_REACH_SLACK_M >= search_reach_m(cfg, bbox_xy, wall);
            good.then_some(cached.product)
        })
        .collect();
    if found.iter().any(Option::is_none) {
        return Ok(None);
    }

    let mut products: Vec<WallProduct> = Vec::with_capacity(found.len());
    for (wall, product) in walls.iter_mut().zip(found) {
        let mut product = product.expect("checked above");
        product.wall_key = wall.key.clone();
        product.building_key = wall.building_key.clone();
        wall.reachable = product.cols > 0;
        products.push(product);
    }

    let mut stats = RunStats {
        buildings: buildings.len(),
        walls: walls.len(),
        cache_hits: products.iter().filter(|p| p.cols > 0).count(),
        ..RunStats::default()
    };
    stats.reachable = stats.cache_hits;
    stats.walls_with_views = stats.cache_hits;
    for p in &products {
        stats.tiers[tier_slot(p.tier)] += 1;
    }
    let credits = record_credits(cfg, &products);
    stats.credited = credits::count();
    let mark = Instant::now();
    let mut result = PipelineResult {
        frame,
        buildings,
        walls,
        products,
        credits,
        export_dir: mint_export_dir(cfg),
        stats,
    };
    write_export(&result, &result.export_dir)?;
    sweep_exports(&cfg.exports_dir(), &result.export_dir, EXPORT_RETENTION);
    result.stats.export_s = mark.elapsed().as_secs_f64();
    let (b, w) = count_exported(&result);
    result.stats.exported_buildings = b;
    result.stats.exported_walls = w;
    Ok(Some(result))
}

/// Records every image whose pixels reached a wall and hands back the credits.
fn record_credits(cfg: &PipelineConfig, products: &[WallProduct]) -> Vec<Credit> {
    let layout = Layout::new(cfg.fetch.cache_dir.clone());
    let used: BTreeSet<&str> = products
        .iter()
        .flat_map(|p| p.views.iter().map(String::as_str))
        .collect();
    used.iter()
        .map(|id| {
            let credit = image_credit(&layout, id, cfg.fetch.area_label.as_deref());
            let out = Credit {
                pano_id: credit.id.clone(),
                title: credit.title.clone(),
                creator: credit.username.clone(),
                creator_url: credit.profile_url(),
                image_url: credit.image_url(),
            };
            credits::record(credit);
            out
        })
        .collect()
}

fn tier_slot(tier: Tier) -> usize {
    match tier {
        Tier::A => 0,
        Tier::B => 1,
        Tier::C => 2,
        Tier::D => 3,
    }
}

/// Every stage after the metadata search, over metadata a caller already has.
///
/// The seam exists because the metadata search is the one stage that cannot be
/// replayed: the fixture holds the Graph records and the Overpass answer of the
/// reference run, so this entry point runs the whole pipeline against them with
/// no network at all.
pub fn run_with(cfg: &PipelineConfig, fetched: &Fetched) -> Result<PipelineResult, String> {
    let t0 = Instant::now();
    let mut stats = RunStats {
        images: fetched.metas.len(),
        clusters: fetched.clusters.len(),
        ..RunStats::default()
    };

    // ---- geometry
    let mark = Instant::now();
    emit_gui_progress_update(
        MESSAGE_ONLY,
        "Mapillary facades: reading reconstructions...",
    );
    let geo = stage_geometry(cfg, fetched)?;
    stats.geometry_s = mark.elapsed().as_secs_f64();
    stats.buildings = geo.buildings.len();
    stats.walls = geo.walls.len();

    // ---- align, which downloads the thumbnails it needs partway through
    cfg.check()?;
    let mark = Instant::now();
    emit_gui_progress_update(MESSAGE_ONLY, "Mapillary facades: registering imagery...");
    let (align, batch) = stage_align(cfg, &geo);
    stats.align_s = mark.elapsed().as_secs_f64();
    stats.images_downloaded = batch.ready.len();
    stats.reachable = align.reachable;
    stats.walls_with_views = align.walls_with_views;
    stats.reg_local = align.stats.local;
    stats.reg_global = align.stats.global;
    stats.reg_none = align.stats.none;
    // Which images this run asked for and did not get. A wall that lost its
    // views to one of these has not been proved blank, only unlucky, and the
    // area record below leaves it out.
    let undelivered: BTreeSet<&str> = batch.failed.iter().map(|(id, _)| id.as_str()).collect();
    let images = batch.ready;

    cfg.check()?;
    let mut geo = geo;
    // The align stage decided reachability from the registered centres, so the
    // walls the export writes carry its answer rather than their own default.
    for wall in &mut geo.walls {
        if let Some(v) = align.views.get(&wall.key) {
            wall.reachable = v.reachable;
            wall.unreachable_reason = v.unreachable_reason.clone();
        }
    }

    // ---- the originals the `hires` ladder will read, which needs the view
    // lists the align stage just decided and so cannot be asked for earlier
    let mark = Instant::now();
    let originals = fetch_originals(cfg, &geo, &align);
    stats.originals_downloaded = originals.len();
    stats.hires_s = mark.elapsed().as_secs_f64();
    stats.image_bytes = bytes_on_disk(images.values().chain(originals.values()));

    // ---- texture
    cfg.check()?;
    let mark = Instant::now();
    let (products, hits, misses) = stage_texture(cfg, &geo, &align, &images, &originals);
    stats.texture_s = mark.elapsed().as_secs_f64();
    stats.cache_hits = hits;
    stats.cache_misses = misses;
    // A cancelled run leaves the walls it never reached as no-view records, so
    // it must not be written over a good export.
    cfg.check()?;
    for p in &products {
        stats.tiers[tier_slot(p.tier)] += 1;
    }
    // Which walls this run proved have no facade, filed per wall beside the
    // walls that have one, so the next generation over these buildings has an
    // answer for all of them and not only for the tenth that carry a texture.
    // Written before the export, because it is only ever an optimisation and a
    // failed write must not fail the run.
    //
    // A wall that lost a candidate to an image this run asked for and did not
    // get is left out. That is not a fact about the wall, it is a download that
    // failed, and filing it would remember a flaky network as "this wall has no
    // facade" for as long as the cache lives. A candidate rejected `no_image`
    // for an image the run never asked for is a different thing: that is this
    // run's own scoping, it repeats, and it may be filed.
    let lost_a_download = |vrec: &WallViews| {
        vrec.candidates.iter().any(|c| {
            c.rejected_reason.as_deref() == Some("no_image")
                && undelivered.contains(c.pano_id.as_str())
        })
    };
    // A run that found no imagery at all does not get to file that answer.
    // "No images" is either genuinely uncovered ground, which costs one cheap
    // search to confirm next time, or a token or a network that failed, which
    // must not be remembered as "these walls have no facade" for as long as the
    // cache lives. The same goes for a search that went partly unanswered: the
    // walls under a refused cell were never given a chance and their tier D is
    // about the search, not about them.
    if fetched.metas.is_empty() {
        eprintln!("Note: no Mapillary imagery for this area; not recording its verdicts");
    } else if fetched.cells_refused > 0 {
        eprintln!(
            "Note: {} Mapillary search cells went unanswered; not recording this area's verdicts",
            fetched.cells_refused
        );
    } else {
        store_blank_verdicts(
            cfg,
            geo.bbox_xy,
            geo.walls
                .iter()
                .zip(&products)
                .filter(|(w, _)| !align.views.get(&w.key).is_some_and(lost_a_download)),
        );
    }

    // ---- credits: every image whose pixels reached a wall
    //
    // Read back off the cached Graph records rather than off this run's search,
    // because a wall served from the facade cache names images the search never
    // returned and CC BY-SA is on those pixels just the same.
    let credits = record_credits(cfg, &products);
    stats.credited = credits::count();

    // ---- export
    let mark = Instant::now();
    let mut result = PipelineResult {
        frame: geo.frame,
        buildings: geo.buildings,
        walls: geo.walls,
        products,
        credits,
        export_dir: mint_export_dir(cfg),
        stats,
    };
    write_export(&result, &result.export_dir)?;
    sweep_exports(&cfg.exports_dir(), &result.export_dir, EXPORT_RETENTION);
    result.stats.export_s = mark.elapsed().as_secs_f64();
    result.stats.exported_buildings = count_exported(&result).0;
    result.stats.exported_walls = count_exported(&result).1;
    result.stats.total_s = t0.elapsed().as_secs_f64();
    Ok(result)
}

fn count_exported(result: &PipelineResult) -> (usize, usize) {
    let mut buildings: BTreeSet<&str> = BTreeSet::new();
    let mut walls = 0usize;
    for p in &result.products {
        if matches!(p.tier, Tier::A | Tier::B) && p.cols > 0 {
            buildings.insert(p.building_key.as_str());
            walls += 1;
        }
    }
    (buildings.len(), walls)
}

/// How one image is credited: the uploader off the cached Graph record's
/// `creator`, the title off the area label, because the Graph API has no title
/// field (see the `fetch` module header).
fn image_credit(layout: &Layout, pano_id: &str, area_label: Option<&str>) -> ImageCredit {
    let creator = fetch::load_meta(layout, pano_id).and_then(|r| r.get("creator").cloned());
    let field = |name: &str| -> String {
        creator
            .as_ref()
            .and_then(|c| c.get(name))
            .map(|v| match v.as_str() {
                Some(s) => s.to_string(),
                None => v.to_string(),
            })
            .unwrap_or_default()
    };
    ImageCredit {
        id: pano_id.to_string(),
        title: match area_label.map(str::trim) {
            Some(label) if !label.is_empty() => label.to_string(),
            _ => format!("Mapillary image {pano_id}"),
        },
        username: field("username"),
        user_id: field("id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapillary::types::{HeightSource, OsmKind};

    fn wall_of(key: &str, edges: Vec<WallEdge>) -> Wall {
        Wall {
            key: key.to_string(),
            building_key: "w1".into(),
            idx: 0,
            node_a: 10,
            node_b: 20,
            a: [0.0, 0.0],
            b: [10.0, 0.0],
            n: [0.0, -1.0],
            length: 10.0,
            merged_idx: vec![0],
            piece: 0,
            n_pieces: 1,
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            reachable: true,
            unreachable_reason: String::new(),
            edges,
            s_offset: 0.0,
        }
    }

    fn product(key: &str, cols: u32, rows: u32) -> WallProduct {
        let cells = (cols * rows) as usize;
        WallProduct {
            wall_key: key.to_string(),
            building_key: "w1".into(),
            node_a: 10,
            node_b: 20,
            piece: 0,
            n_pieces: 1,
            edges: Vec::new(),
            col0_m: 1.0,
            cols,
            rows,
            rgb: vec![[200, 190, 180]; cells],
            cls: vec![super::super::openings::CLS_WALL; cells],
            observed: vec![true; cells],
            bands: vec![[200, 190, 180]; rows as usize],
            tex: None,
            tier: Tier::A,
            confidence: 0.8,
            height_used_m: f64::from(rows),
            unknown_share: 0.0,
            views: vec!["123".into()],
            flags: Vec::new(),
        }
    }

    #[test]
    fn the_cache_key_is_the_walls_node_ids() {
        let plain = wall_of("w1_0", Vec::new());
        assert_eq!(wall_node_ids(&plain), vec![10, 20]);
        let merged = wall_of(
            "w1_0",
            vec![
                WallEdge {
                    edge_idx: 0,
                    node_a: 10,
                    node_b: 11,
                    s0: 0.0,
                    s1: 4.0,
                },
                WallEdge {
                    edge_idx: 1,
                    node_a: 11,
                    node_b: 20,
                    s0: 4.0,
                    s1: 10.0,
                },
            ],
        );
        assert_eq!(wall_node_ids(&merged), vec![10, 11, 20]);
        // The same identity `cache::wall_node_ids` derives from a product, so a
        // wall found in the cache is the wall that was put there.
        let mut p = product("w1_0", 10, 12);
        p.edges = merged.edges.clone();
        assert_eq!(cache::wall_node_ids(&p), wall_node_ids(&merged));
        // Every piece of a split span carries the whole span's edges, so the
        // node ids alone would hand the second piece the first one's facade.
        let ids = wall_node_ids(&merged);
        assert_ne!(
            cache::wall_cache_key(&ids, 0),
            cache::wall_cache_key(&ids, 1),
            "two pieces of one span must not share a cache entry"
        );
    }

    #[test]
    fn a_wall_record_carries_what_the_consumer_parses() {
        let wall = wall_of("w1_0", Vec::new());
        let frame = Frame::new(11.5795305, 48.13643);
        let rec = wall_record(&product("w1_0", 10, 12), &wall, &frame, 1);
        assert_eq!(rec["key"], "w1_0");
        assert_eq!(rec["cols"], 10);
        assert_eq!(rec["rows"], 12);
        assert_eq!(rec["tier"], "A");
        assert_eq!(rec["png"], "w1_0.png");
        assert_eq!(rec["tex"], Value::Null, "no texture, no texture file");
        assert!((rec["extent"]["s_l"].as_f64().unwrap() - 1.0).abs() < 1e-12);
        let edges = rec["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1, "a wall with no ring edges is one edge");
        assert_eq!(edges[0]["node_a"], 10);
        assert_eq!(edges[0]["col0"], 0);
    }

    #[test]
    fn a_wall_below_tier_b_names_no_files() {
        let wall = wall_of("w1_0", Vec::new());
        let frame = Frame::new(11.5795305, 48.13643);
        let mut p = product("w1_0", 10, 12);
        p.tier = Tier::C;
        let rec = wall_record(&p, &wall, &frame, 1);
        assert_eq!(rec["png"], Value::Null);
        assert_eq!(rec["cols"], Value::Null);
        assert_eq!(rec["tier"], "C");
    }

    #[test]
    fn the_weighted_median_is_the_lower_one() {
        // Half the weight sits on 1.0, so the sample at the halfway mark is 1.0
        // and not the interpolated 1.5.
        assert!((lower_weighted_median(&[1.0, 2.0], &[1.0, 1.0]) - 1.0).abs() < 1e-12);
        assert!((lower_weighted_median(&[1.0, 2.0], &[0.1, 1.0]) - 2.0).abs() < 1e-12);
        assert!((lower_weighted_median(&[3.0, 1.0, 2.0], &[1.0, 1.0, 1.0]) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn a_building_colour_ignores_foliage_and_sky() {
        let mut p = product("w1_0", 4, 8);
        // green, and bright blue, on the first row of the band
        for i in 0..4 {
            p.rgb[4 * 4 + i] = [40, 160, 60];
        }
        let (colour, conf) = building_colour(&[(&p, 0.8)]);
        let c = colour.expect("some wall cell survives");
        assert!(c[1] < 200 && c[0] > 150, "the beige wall wins: {c:?}");
        assert!(conf > 0.0 && conf <= 0.8);
    }

    #[test]
    fn the_export_writes_what_install_reads() {
        let dir = std::env::temp_dir().join(format!("arnis-facade-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let building = Building {
            key: "w1".into(),
            osm_id: 1,
            kind: OsmKind::Way,
            ring: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            holes: Vec::new(),
            node_ids: vec![10, 20, 30, 40],
            tags: Default::default(),
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            min_height: 0.0,
            target: true,
            member_ways: Vec::new(),
        };
        let result = PipelineResult {
            buildings: vec![building],
            walls: vec![wall_of("w1_0", Vec::new())],
            products: vec![product("w1_0", 10, 12)],
            credits: vec![Credit {
                pano_id: "111".into(),
                title: "Ledererstrasse, Munich".into(),
                creator: "nunocaldeira".into(),
                creator_url: "https://www.mapillary.com/app/user/nunocaldeira".into(),
                image_url: "https://www.mapillary.com/app/?pKey=111&focus=photo".into(),
            }],
            ..PipelineResult::default()
        };
        write_export(&result, &dir).expect("export");
        assert!(dir.join("w1.json").exists());
        assert!(dir.join("w1_0.png").exists());
        assert!(dir.join("w1_0_cls.png").exists());
        let png = image::open(dir.join("w1_0.png")).unwrap().to_rgba8();
        assert_eq!(png.dimensions(), (10, 12), "1 px per metre");
        assert_eq!(png.get_pixel(0, 0).0[3], super::super::openings::CLS_WALL);
        let rec: Value =
            serde_json::from_slice(&std::fs::read(dir.join("w1.json")).unwrap()).unwrap();
        assert_eq!(rec["kind"], "way");
        assert_eq!(rec["way_id"], 1);
        assert_eq!(rec["relation_id"], Value::Null);
        assert_eq!(rec["exported_walls"], 1);
        assert!(rec["building_colour"]["rgb"].is_array());
        // The attribution goes with the export, so a folder handed back to
        // `--mapillary-facades-dir` long after the run can still name the
        // photographers rather than only the photographs.
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["credits"][0]["id"], "111");
        assert_eq!(manifest["credits"][0]["creator"], "nunocaldeira");
        assert_eq!(manifest["credits"][0]["title"], "Ledererstrasse, Munich");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------- the export directory

    /// A config on a scratch cache, so nothing here touches the user's.
    fn scratch_cfg(dir: &Path, bbox: super::super::types::BBox) -> PipelineConfig {
        let mut fetch = FetchConfig::new("MLY|test", bbox);
        fetch.cache_dir = dir.to_path_buf();
        PipelineConfig::new(fetch, Params::default())
    }

    fn one_building_result() -> PipelineResult {
        PipelineResult {
            buildings: vec![Building {
                key: "w1".into(),
                osm_id: 1,
                kind: OsmKind::Way,
                ring: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
                holes: Vec::new(),
                node_ids: vec![10, 20, 30, 40],
                tags: Default::default(),
                height_osm: Some(12.0),
                height_source: HeightSource::Tag,
                min_height: 0.0,
                target: true,
                member_ways: Vec::new(),
            }],
            walls: vec![wall_of("w1_0", Vec::new())],
            products: vec![product("w1_0", 10, 12)],
            ..PipelineResult::default()
        }
    }

    /// Two generations at once must keep their own facades.
    ///
    /// There used to be one `export/` per params digest, cleared at the start of
    /// every write, so whichever run wrote second destroyed the first one's
    /// buildings and whichever wrote its manifest last claimed the other's
    /// files. It left the owner's cache with a manifest saying 0 buildings and 0
    /// walls sitting beside 407 data files from a different run, and that
    /// manifest is where the export's image credits live, so the world built
    /// from it credited nobody for imagery that is CC BY-SA.
    #[test]
    fn two_runs_at_once_keep_their_own_export() {
        let tmp = tempfile::tempdir().unwrap();
        let bbox = super::super::types::BBox::new(48.135635, 11.578243, 48.137225, 11.580818);
        let cfg = scratch_cfg(tmp.path(), bbox);

        // The same config, the same digest, the same area: everything that used
        // to make two runs share one directory.
        let full = one_building_result();
        let empty = PipelineResult::default();
        let gate = std::sync::Barrier::new(2);
        let (a, b) = std::thread::scope(|s| {
            let one = s.spawn(|| {
                let dir = mint_export_dir(&cfg);
                gate.wait();
                write_export(&full, &dir).expect("export");
                dir
            });
            let two = s.spawn(|| {
                let dir = mint_export_dir(&cfg);
                gate.wait();
                write_export(&empty, &dir).expect("export");
                dir
            });
            (one.join().unwrap(), two.join().unwrap())
        });

        assert_ne!(a, b, "two runs must not be handed one directory");
        // The run that found a building still has it, and its manifest counts
        // its own walls and not the other run's.
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(a.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["buildings"], 1);
        assert_eq!(manifest["walls"], 1);
        assert!(a.join("w1.json").exists());
        assert!(a.join("w1_0.png").exists());
        // And the run that found nothing has an empty export of its own, not a
        // wiped copy of the other one's.
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(b.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["buildings"], 0);
        assert!(!b.join("w1.json").exists());
    }

    /// The claim is the mechanism, not the nonce: a caller that hands one
    /// directory to two runs is told so rather than getting one run's images
    /// under another run's manifest.
    #[test]
    fn an_export_directory_is_claimed_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("exports").join("run");
        write_export(&one_building_result(), &dir).expect("the first run claims it");
        let second = write_export(&PipelineResult::default(), &dir);
        assert!(second.is_err(), "the second run must be refused");
        // And the first run's export is untouched by the refusal.
        assert!(dir.join("w1.json").exists());
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["walls"], 1);
    }

    #[test]
    fn the_sweep_spares_this_run_and_everything_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let exports = tmp.path().join("exports");
        // Two exports of one box, named as `mint_export_dir` names them: the
        // sweep keeps the newest of each area for the preview, so a pair from
        // different areas would both survive and say nothing about retention.
        let keep = exports.join("a1b2-2-1-1");
        let other = exports.join("a1b2-1-1-0");
        for d in [&keep, &other] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("manifest.json"), b"{}").unwrap();
        }

        // Nothing here is an hour old, so a run that has just finished takes
        // nothing away from a generation running beside it.
        sweep_exports(&exports, &keep, EXPORT_RETENTION);
        assert!(keep.exists() && other.exists());

        // With everything old enough, this run's own export is still spared and
        // the rest go.
        sweep_exports(&exports, &keep, std::time::Duration::ZERO);
        assert!(keep.exists(), "a run never sweeps its own export");
        assert!(!other.exists());
    }

    /// The 3D preview reads these directories, so an area that was precomputed
    /// yesterday has to still have one, whatever has been generated since.
    #[test]
    fn the_sweep_keeps_the_newest_export_of_every_area() {
        let tmp = tempfile::tempdir().unwrap();
        let exports = tmp.path().join("exports");
        // Two areas, two exports each, plus the run doing the sweeping.
        let old_a = exports.join("aaaa-1-1-0");
        let new_a = exports.join("aaaa-2-1-1");
        let old_b = exports.join("bbbb-1-1-2");
        let new_b = exports.join("bbbb-2-1-3");
        let keep = exports.join("cccc-9-1-4");
        for d in [&old_a, &new_a, &old_b, &new_b, &keep] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("manifest.json"), b"{}").unwrap();
            // The name says which is newer; the file system's own timestamps
            // are milliseconds apart and would order these by creation.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        sweep_exports(&exports, &keep, std::time::Duration::ZERO);
        assert!(keep.exists(), "a run never sweeps its own export");
        assert!(
            new_a.exists(),
            "the newest of an area survives for the preview"
        );
        assert!(new_b.exists(), "and so does the newest of every other area");
        assert!(!old_a.exists(), "the runs it superseded do not");
        assert!(!old_b.exists());
    }

    /// Sparing one export per area cannot mean sparing one per box the user has
    /// ever drawn, or a year of generating fills the cache directory.
    #[test]
    fn the_sweep_spares_only_the_last_few_areas() {
        let tmp = tempfile::tempdir().unwrap();
        let exports = tmp.path().join("exports");
        let mut areas = Vec::new();
        for i in 0..EXPORT_KEEP_AREAS + 3 {
            let dir = exports.join(format!("area{i:04x}-1-1-{i:x}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("manifest.json"), b"{}").unwrap();
            areas.push(dir);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let keep = exports.join("mine-9-1-0");
        std::fs::create_dir_all(&keep).unwrap();

        sweep_exports(&exports, &keep, std::time::Duration::ZERO);
        let left = areas.iter().filter(|d| d.exists()).count();
        assert_eq!(
            left, EXPORT_KEEP_AREAS,
            "only the most recent areas survive"
        );
        // And they are the most recent ones, not an arbitrary handful.
        for dir in areas.iter().take(3) {
            assert!(!dir.exists(), "{} is the oldest and must go", dir.display());
        }
    }

    // ------------------------------------------------------- reuse across bboxes

    /// A square building as Overpass would hand it over, `size_m` on a side,
    /// centred on `lon`/`lat`.
    fn osm_square(way_id: i64, lon: f64, lat: f64, size_m: f64) -> Vec<Value> {
        let dlat = 0.5 * size_m / 111_195.0;
        let dlon = dlat / lat.to_radians().cos();
        let node0 = way_id * 100;
        let corners = [
            (lon - dlon, lat - dlat),
            (lon + dlon, lat - dlat),
            (lon + dlon, lat + dlat),
            (lon - dlon, lat + dlat),
        ];
        let mut out: Vec<Value> = corners
            .iter()
            .enumerate()
            .map(|(i, (lon, lat))| {
                json!({"type": "node", "id": node0 + i as i64, "lon": lon, "lat": lat})
            })
            .collect();
        let mut ids: Vec<i64> = (0..4).map(|i| node0 + i).collect();
        ids.push(node0);
        out.push(json!({
            "type": "way",
            "id": way_id,
            "nodes": ids,
            "tags": {"building": "yes", "height": "12"},
        }));
        out
    }

    /// The Munich test box, and the same box nudged by 22 cm.
    fn munich_box() -> super::super::types::BBox {
        super::super::types::BBox::new(48.135635, 11.578243, 48.137225, 11.580818)
    }

    fn nudged(b: super::super::types::BBox, metres: f64) -> super::super::types::BBox {
        let dlat = metres / 111_195.0;
        super::super::types::BBox::new(b.min_lat + dlat, b.min_lon, b.max_lat + dlat, b.max_lon)
    }

    /// Files a "no facade" verdict for every wall of `cfg`'s buildings, the way
    /// a run that had done the align work would.
    fn file_verdicts(cfg: &PipelineConfig, osm: &Value) -> usize {
        let (frame, _, walls) = footprints_and_walls(cfg, osm);
        let bbox_xy = bbox_in_frame(cfg, &frame);
        let products: Vec<WallProduct> = walls
            .iter()
            .map(|w| no_view_product(w, &WallViews::default()))
            .collect();
        store_blank_verdicts(cfg, bbox_xy, walls.iter().zip(&products));
        walls.len()
    }

    /// The whole point of keying the cache by the wall: a box the user nudged is
    /// not a different area.
    ///
    /// The verdicts used to be filed under a hash of the four bbox corners, so a
    /// box moved by 22 cm found no record at all and repeated the area's align
    /// work for walls that were every one of them already decided.
    #[test]
    fn a_box_moved_a_hand_span_reuses_the_walls_it_already_has() {
        let tmp = tempfile::tempdir().unwrap();
        let osm = json!({"elements": osm_square(1, 11.5795, 48.1364, 20.0)});
        let first = scratch_cfg(tmp.path(), munich_box());
        let walls = file_verdicts(&first, &osm);
        assert!(walls > 0, "the fixture must have walls");

        // The same box: the cache answers, as it always did.
        let same = run_from_cache(&first, &osm)
            .expect("the cache path")
            .expect("a box just filed must be answered");
        assert_eq!(same.stats.walls, walls);

        // And the box moved 22 cm, which is a different area record and the same
        // walls. This is the case that used to redo the align stage.
        let moved = scratch_cfg(tmp.path(), nudged(munich_box(), 0.22));
        let after = run_from_cache(&moved, &osm)
            .expect("the cache path")
            .expect("a nudged box must reuse the walls it already has");
        assert_eq!(after.stats.walls, walls);
        assert_eq!(after.stats.tiers, same.stats.tiers);
        // Each run wrote its own export, so neither took the other's.
        assert_ne!(same.export_dir, after.export_dir);
        assert!(after.export_dir.join("manifest.json").exists());
    }

    /// A generation whose box is too large for a cold run must not start one.
    ///
    /// `data_processing` joins the facade job before it builds a single
    /// building, so a cold pipeline is the world waiting, and a box thirty times
    /// the cap is hours of it with no cancel button. Cache-only says so at once
    /// instead, and does not even ask Overpass when nothing has ever been built.
    #[test]
    fn a_box_too_large_for_a_cold_run_refuses_instead_of_fetching() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = scratch_cfg(tmp.path(), munich_box());
        cfg.cache_only = Some("too large, precompute it in pieces".to_string());
        assert!(!cfg.facade_cache.exists(), "the fixture starts cold");

        // `run` and not `run_from_osm`: with nothing built there is no reason to
        // ask Overpass either, so this returns before the first request. A test
        // that reached the network would hang here rather than fail.
        let err = run(&cfg).expect_err("a cold cache-only run cannot answer");
        assert_eq!(err, "too large, precompute it in pieces");

        // And with buildings on record but no verdict for them, it still refuses
        // rather than falling through to the imagery search.
        let osm = json!({"elements": osm_square(1, 11.5795, 48.1364, 20.0)});
        std::fs::create_dir_all(&cfg.facade_cache).unwrap();
        let err = run_from_osm(&cfg, osm, Instant::now())
            .expect_err("an unbuilt area cannot be served from the cache");
        assert_eq!(err, "too large, precompute it in pieces");
    }

    /// Cache-only is not "no facades": an area precomputed in pieces is exactly
    /// what it exists to serve, and it must still export the whole run.
    ///
    /// This is the way out the refusal above tells the user about, so it has to
    /// work or the advice is wrong.
    #[test]
    fn a_precomputed_area_still_exports_under_the_cache_only_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let osm = json!({"elements": osm_square(1, 11.5795, 48.1364, 20.0)});
        let precompute = scratch_cfg(tmp.path(), munich_box());
        let walls = file_verdicts(&precompute, &osm);
        assert!(walls > 0, "the fixture must have walls");

        let mut generation = scratch_cfg(tmp.path(), munich_box());
        generation.cache_only = Some("would have been refused".to_string());
        let result = run_from_osm(&generation, osm, Instant::now())
            .expect("the cache holds every wall of this area");
        assert_eq!(result.stats.walls, walls);
        assert_eq!(result.stats.cache_misses, 0, "nothing was built");
        assert!(result.export_dir.join("manifest.json").exists());
    }

    /// Two pieces precomputed side by side answer the box that covers both.
    ///
    /// This is the whole of the advice a box over the cap is given, so it is
    /// worth a test of its own rather than a corollary of the one above. It is
    /// also what the reach rule in `cache::store_wall` exists for: each piece
    /// judges the other's walls as occluders and reaches nothing round them, so
    /// filing that would put a zero over the proof its neighbour had paid for
    /// and leave the wide box with no answer along the seam.
    #[test]
    fn an_area_precomputed_in_two_pieces_answers_the_box_that_covers_both() {
        let tmp = tempfile::tempdir().unwrap();
        // A building in each half, 40 m apart, which is inside `osm_margin_m`
        // of the other half's box: each piece sees the other's wall.
        let mut elements = osm_square(1, 11.5795, 48.13580, 20.0);
        elements.extend(osm_square(2, 11.5795, 48.13640, 20.0));
        let osm = json!({ "elements": elements });

        let wide = super::super::types::BBox::new(48.13550, 11.5790, 48.13670, 11.5800);
        let south = super::super::types::BBox::new(48.13550, 11.5790, 48.13610, 11.5800);
        let north = super::super::types::BBox::new(48.13610, 11.5790, 48.13670, 11.5800);

        // Precomputed one piece at a time, the way the refusal tells the user to.
        let walls = file_verdicts(&scratch_cfg(tmp.path(), south), &osm);
        assert_eq!(file_verdicts(&scratch_cfg(tmp.path(), north), &osm), walls);

        let mut generation = scratch_cfg(tmp.path(), wide);
        generation.cache_only = Some("would have been refused".to_string());
        let result = run_from_osm(&generation, osm, Instant::now())
            .expect("both pieces together cover the box");
        assert_eq!(result.stats.walls, walls);
        assert_eq!(result.stats.cache_misses, 0);
    }

    /// The one thing the bbox still decides: whether this run searched enough of
    /// a wall's surroundings for "no facade" to be a fact about the wall.
    ///
    /// A building outside the box is only an occluder, and the imagery search
    /// stops `pano_margin_m` beyond the box, so a verdict on it says only that
    /// this box did not look. A later box that does contain it must do the work
    /// rather than believe that.
    #[test]
    fn a_verdict_from_half_a_look_is_not_reused_by_a_box_that_looks_properly() {
        let tmp = tempfile::tempdir().unwrap();
        // 150 m north of the box's top edge: inside the OSM margin of a box
        // reaching that far, outside the imagery search of this one.
        let far = json!({"elements": osm_square(2, 11.5795, 48.1385, 20.0)});
        let inside = json!({"elements": osm_square(1, 11.5795, 48.1364, 20.0)});
        let mut both = inside["elements"].as_array().unwrap().clone();
        both.extend(far["elements"].as_array().unwrap().clone());
        let both = json!({"elements": both});

        let small = scratch_cfg(tmp.path(), munich_box());
        file_verdicts(&small, &both);

        // The small box settled the building inside it and could not settle the
        // one 150 m away, so it may answer for itself and not for a wider box.
        assert!(run_from_cache(&small, &both).unwrap().is_some());

        let wide = scratch_cfg(
            tmp.path(),
            super::super::types::BBox::new(48.135635, 11.578243, 48.139, 11.580818),
        );
        assert!(
            run_from_cache(&wide, &both).unwrap().is_none(),
            "a box that now contains the far building must go and look at it"
        );
    }

    #[test]
    fn every_wall_a_world_can_contain_is_searched_to_the_full_camera_reach() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = scratch_cfg(tmp.path(), munich_box());
        let reach = cfg.params.far_dist_m;
        assert!(
            (cfg.fetch.pano_margin_m - reach).abs() < 1e-9,
            "the defaults search exactly as far as a camera may be from a wall"
        );
        let frame = geometry::build_frame(cfg.fetch.bbox);
        let bbox_xy = bbox_in_frame(&cfg, &frame);

        let mut wall = wall_of("w1_0", Vec::new());
        wall.a = [0.0, 0.0];
        wall.b = [10.0, 0.0];
        assert!(
            (search_reach_m(&cfg, bbox_xy, &wall) - reach).abs() < 1e-9,
            "a wall in the box is judged against every camera that could see it"
        );
        // On the box edge is still the cap, because the search reaches exactly
        // one camera range beyond the box.
        wall.a = [bbox_xy[2], 0.0];
        wall.b = [bbox_xy[2] - 10.0, 0.0];
        assert!((search_reach_m(&cfg, bbox_xy, &wall) - reach).abs() < 1e-9);

        // Ten metres outside it, where a camera the search never reached could
        // still have seen the wall, and the shortfall is what says so.
        wall.a = [bbox_xy[2] + 10.0, 0.0];
        wall.b = [bbox_xy[2] + 20.0, 0.0];
        assert!((search_reach_m(&cfg, bbox_xy, &wall) - (reach - 20.0)).abs() < 1e-9);

        // Past the search area entirely, which is no reach at all rather than a
        // negative one.
        wall.a = [bbox_xy[2] + 60.0, 0.0];
        wall.b = [bbox_xy[2] + 70.0, 0.0];
        assert_eq!(search_reach_m(&cfg, bbox_xy, &wall), 0.0);
    }

    // ----------------------------------------------------------------- replay

    /// The Munich fixture laid out the way the cache expects it.
    ///
    /// `cache_munich_test/` holds the reference run's own inputs under the
    /// Python lab's names; the four trees here are hard links to those exact
    /// bytes, so the replay reads what the reference read and never reaches the
    /// network. Built once and left in place, because it is 5 GB of imagery by
    /// name and nothing by content.
    fn replay_cache() -> Option<PathBuf> {
        let lab = crate::mapillary::golden::lab_cache_dir()?;
        let root = std::env::temp_dir()
            .join("arnis-facade-replay")
            .join("mapillary");
        let link = |src: PathBuf, dst: PathBuf| {
            if dst.exists() || !src.exists() {
                return;
            }
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::hard_link(&src, &dst).is_err() {
                let _ = std::fs::copy(&src, &dst);
            }
        };
        let shard = |key: &str| -> String {
            let n = key.len();
            key[n.saturating_sub(2)..].to_string()
        };
        for (dir, suffix, want) in [
            ("thumbs", "_2048.jpg", "_2048.jpg"),
            ("panos", ".jpg", "_orig.jpg"),
            ("sfm", ".json", ".json.zz"),
        ] {
            let Ok(entries) = std::fs::read_dir(lab.join(dir)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Some(key) = name.strip_suffix(suffix) else {
                    continue;
                };
                let tree = if dir == "sfm" { "sfm" } else { "img" };
                link(
                    entry.path(),
                    root.join(tree)
                        .join(shard(key))
                        .join(format!("{key}{want}")),
                );
            }
        }
        // The Graph records, one file each, which is where the pipeline reads
        // the signed URLs and the credit fields back from.
        let panos: Value =
            serde_json::from_slice(&std::fs::read(lab.join("panos.json")).ok()?).ok()?;
        for record in panos.get("panos")?.as_array()? {
            let Some(id) = record.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let path = root.join("meta").join(shard(id)).join(format!("{id}.json"));
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&path, serde_json::to_vec(record).ok()?);
            }
        }
        Some(root)
    }

    /// The reference run's inputs as the fetch stage would have handed them
    /// over: every Graph record it consumed, the clusters that were actually
    /// cached (the other sixteen were already unavailable when the reference
    /// ran), and the Overpass answer.
    fn replay_fetched(root: &Path) -> Option<Fetched> {
        let lab = crate::mapillary::golden::lab_cache_dir()?;
        let panos: Value =
            serde_json::from_slice(&std::fs::read(lab.join("panos.json")).ok()?).ok()?;
        let layout = Layout::new(root.to_path_buf());
        let mut fetched = Fetched::default();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for record in panos.get("panos")?.as_array()? {
            let Some(meta) = PanoMeta::from_graph(record) else {
                continue;
            };
            if let Some(id) = meta.cluster_id.clone() {
                let cached = layout
                    .cluster_path(&id)
                    .map(|p| p.exists())
                    .unwrap_or(false);
                if cached && seen.insert(id.clone()) {
                    fetched.clusters.push(super::super::fetch::ClusterRef {
                        id,
                        pano_id: meta.id.clone(),
                    });
                }
            }
            fetched.credits.push(Credit {
                pano_id: meta.id.clone(),
                title: "Munich".to_string(),
                creator: String::new(),
                creator_url: String::new(),
                image_url: String::new(),
            });
            fetched.metas.push(meta);
        }
        fetched.osm = serde_json::from_slice(&std::fs::read(lab.join("osm.json")).ok()?).ok()?;
        Some(fetched)
    }

    /// One exported wall as the comparison reads it.
    struct ExportedWall {
        tier: String,
        cols: u32,
        rows: u32,
        /// The block grid, RGB with the class in alpha.
        cells: Vec<[u8; 4]>,
        /// The 8 px per metre texture, RGB with the valid mask in alpha.
        tex: Option<(u32, u32, Vec<[u8; 4]>)>,
        s_l: f64,
        height_used_m: f64,
    }

    impl ExportedWall {
        /// Per row, the median colour of the row's wall cells, which is the
        /// band colour the tolerance is stated in.
        fn bands(&self) -> Vec<Option<[u8; 3]>> {
            (0..self.rows as usize)
                .map(|r| {
                    let mut chan: [Vec<u8>; 3] = Default::default();
                    for c in 0..self.cols as usize {
                        let p = self.cells[r * self.cols as usize + c];
                        if p[3] == super::super::openings::CLS_WALL {
                            for k in 0..3 {
                                chan[k].push(p[k]);
                            }
                        }
                    }
                    if chan[0].is_empty() {
                        return None;
                    }
                    let mut out = [0u8; 3];
                    for (k, o) in out.iter_mut().enumerate() {
                        chan[k].sort_unstable();
                        *o = chan[k][chan[k].len() / 2];
                    }
                    Some(out)
                })
                .collect()
        }
    }

    /// Every tier A or B wall of an export directory.
    fn read_export(dir: &Path) -> BTreeMap<String, ExportedWall> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json")
                || path.file_name().and_then(|n| n.to_str()) == Some("manifest.json")
            {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(rec) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            for w in rec["walls"].as_array().into_iter().flatten() {
                let tier = w["tier"].as_str().unwrap_or("D").to_string();
                if !matches!(tier.as_str(), "A" | "B") {
                    continue;
                }
                let (Some(key), Some(png)) = (w["key"].as_str(), w["png"].as_str()) else {
                    continue;
                };
                let Ok(img) = image::open(dir.join(png)) else {
                    continue;
                };
                let img = img.to_rgba8();
                let tex = w["tex"]
                    .as_str()
                    .and_then(|t| image::open(dir.join(t)).ok())
                    .map(|t| {
                        let t = t.to_rgba8();
                        (t.width(), t.height(), t.pixels().map(|p| p.0).collect())
                    });
                out.insert(
                    key.to_string(),
                    ExportedWall {
                        tier,
                        cols: img.width(),
                        rows: img.height(),
                        cells: img.pixels().map(|p| p.0).collect(),
                        tex,
                        s_l: w["extent"]["s_l"].as_f64().unwrap_or(f64::NAN),
                        height_used_m: w["height_used_m"].as_f64().unwrap_or(f64::NAN),
                    },
                );
            }
        }
        out
    }

    /// What a box the user nudged costs, measured on the Munich fixture, both
    /// ways round. Run it after `the_driver_reproduces_the_python_run_on_munich`
    /// has left the replay cache warm:
    ///
    /// ```text
    /// cargo test --target-dir target-plane -- --ignored --nocapture nudged_box_costs
    /// ```
    #[test]
    #[ignore = "needs the warm replay cache the Munich driver test leaves behind"]
    fn what_a_nudged_box_costs() {
        let Some(root) = replay_cache() else {
            eprintln!("no lab cache; skipped");
            return;
        };
        let fetched = replay_fetched(&root).expect("the fixture's own inputs");
        let bbox = nudged(munich_box(), 0.22);
        let mut fetch_cfg = FetchConfig::new("replay", bbox);
        fetch_cfg.cache_dir = root.clone();
        fetch_cfg.area_label = Some("Munich".to_string());
        let cfg = PipelineConfig::new(fetch_cfg, Params::default());
        if !cfg.facade_cache.exists() {
            eprintln!("the facade cache is cold; run the driver test first");
            return;
        }

        let t = Instant::now();
        let reused = run_from_cache(&cfg, &fetched.osm)
            .expect("the cache path")
            .expect("a box moved 22 cm holds the same walls");
        let cheap = t.elapsed().as_secs_f64();

        // What the same nudge used to cost: the area record was filed under the
        // four bbox corners, so it was missed and the run went the long way
        // round, doing the whole align stage for walls that were every one of
        // them already decided.
        let t = Instant::now();
        let full = run_with(&cfg, &fetched).expect("the pipeline runs");
        let dear = t.elapsed().as_secs_f64();

        println!(
            "nudged, from the cache: {:.1} s  {}",
            cheap,
            reused.stats.summary()
        );
        println!(
            "nudged, the long way:   {:.1} s  {}",
            dear,
            full.stats.summary()
        );
        println!(
            "the nudge now costs {:.1} s instead of {:.1} s, a factor of {:.0}, \
             for {} exported walls against {}",
            cheap,
            dear,
            dear / cheap.max(1e-9),
            reused.stats.exported_walls,
            full.stats.exported_walls
        );
        // The two are not asserted equal, and that is the trade this makes.
        // Measured on 2026-09-06 the cache path exports 111 walls and a fresh
        // align of the nudged box exports 112: one wall of 1030 changes its
        // verdict under 22 cm, because `register::run_align` hands `bbox_xy` to
        // `register_cluster_global` and 367 of these 1076 panos are registered
        // by that fallback, so the box moves a camera and a gate value with it.
        // Reusing costs that wall and saves 1154 seconds; doing the work again
        // to find it would be doing it on every nudge, for every wall.
        assert_eq!(reused.stats.walls, full.stats.walls, "the same wall set");
        assert_eq!(
            reused.stats.cache_misses, 0,
            "the cache path builds nothing"
        );
        assert_ne!(reused.export_dir, full.export_dir);
    }

    /// The whole driver against the Munich fixture, compared with the Python
    /// reference run wall by wall.
    ///
    /// Ignored because it is the reference run: it wants the 5 GB lab cache and
    /// takes minutes. Run it with
    ///
    /// ```text
    /// cargo test --target-dir target-test -- --ignored --nocapture munich
    /// ```
    #[test]
    #[ignore = "needs tools/facade_lab/cache_munich_test and takes minutes"]
    fn the_driver_reproduces_the_python_run_on_munich() {
        if crate::mapillary::golden::pixels_absent() {
            return;
        }

        let Some(root) = replay_cache() else {
            eprintln!("no lab cache; skipped");
            return;
        };
        let fetched = replay_fetched(&root).expect("the fixture's own inputs");
        let bbox = super::super::types::BBox::new(48.135635, 11.578243, 48.137225, 11.580818);
        let mut fetch_cfg = FetchConfig::new("replay", bbox);
        fetch_cfg.cache_dir = root.clone();
        fetch_cfg.area_label = Some("Munich".to_string());
        let cfg = PipelineConfig::new(fetch_cfg, Params::default());

        // First pass: an empty facade tree, so every wall is built. A few walls
        // also dump everything they were made of, which is the only way to see
        // that path run over real imagery. Beside `r147094_8p0`, which is there
        // to exercise the dump, the list is the three walls this comparison
        // disagrees with the reference on by the most metres of height, so the
        // run leaves the gate table and the crops behind for exactly the walls
        // somebody will want to look at next.
        let _ = std::fs::remove_dir_all(&cfg.facade_cache);
        let dump = std::env::temp_dir().join("arnis-facade-debug");
        let _ = std::fs::remove_dir_all(&dump);
        let cold = run_with(
            &PipelineConfig {
                debug: Some(DebugDump {
                    dir: dump.clone(),
                    walls: vec![
                        "r147094_8p0".to_string(),
                        "w81190185_2".to_string(),
                        "w81190199_0".to_string(),
                        "w79817227_4".to_string(),
                    ],
                }),
                ..cfg.clone()
            },
            &fetched,
        )
        .expect("the pipeline runs");
        println!("cold: {}", cold.stats.summary());
        for name in [
            "r147094_8p0.json",
            "r147094_8p0_tex.png",
            "r147094_8p0_blocks.png",
            "r147094_8p0_gates.tsv",
        ] {
            assert!(
                dump.join(name).exists(),
                "the debug dump owes {name}: {:?}",
                std::fs::read_dir(&dump)
                    .map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
            );
        }

        // Second pass: the same area again, which must build no wall.
        let warm = run_with(&cfg, &fetched).expect("the pipeline runs again");
        println!("warm: {}", warm.stats.summary());
        assert_eq!(
            warm.stats.cache_misses, 0,
            "a generation over an area already built must do no image work"
        );
        assert_eq!(
            warm.stats.tiers, cold.stats.tiers,
            "the cache must hand back the tiers it stored"
        );

        // Third pass: the path `run` takes when the cache already answers the
        // whole box, which never opens a photograph or registers a camera.
        let t = Instant::now();
        let cached = run_from_cache(&cfg, &fetched.osm)
            .expect("the cache path")
            .expect("every wall of a box just built is in the cache");
        println!(
            "cached: {}  [{:.1} s]",
            cached.stats.summary(),
            t.elapsed().as_secs_f64()
        );
        assert_eq!(
            cached.stats.tiers, warm.stats.tiers,
            "the cache-only path must reproduce the run's own tiers"
        );
        assert_eq!(cached.stats.exported_walls, warm.stats.exported_walls);
        assert_eq!(cached.stats.credited, warm.stats.credited);

        // Fourth pass: the same area with the box nudged 22 cm north, which is
        // the case the per bbox area record used to miss entirely. Every wall is
        // the same wall, so every one of them is already decided and the run
        // does no align work at all.
        let moved = PipelineConfig {
            fetch: FetchConfig {
                bbox: nudged(bbox, 0.22),
                ..cfg.fetch.clone()
            },
            ..cfg.clone()
        };
        let t = Instant::now();
        let nudged_run = run_from_cache(&moved, &fetched.osm)
            .expect("the cache path")
            .expect("a box moved 22 cm holds the same walls");
        println!(
            "nudged: {}  [{:.1} s]",
            nudged_run.stats.summary(),
            t.elapsed().as_secs_f64()
        );
        assert_eq!(nudged_run.stats.walls, cached.stats.walls);
        assert_eq!(nudged_run.stats.tiers, cached.stats.tiers);
        assert_eq!(
            nudged_run.stats.exported_walls, cached.stats.exported_walls,
            "a nudged box must carry the same facades, not fewer"
        );
        // Four runs, four exports, none of them written over another's.
        for (a, b) in [
            (&cold.export_dir, &warm.export_dir),
            (&warm.export_dir, &cached.export_dir),
            (&cached.export_dir, &nudged_run.export_dir),
        ] {
            assert_ne!(a, b);
            assert!(a.join("manifest.json").exists(), "{a:?} lost its manifest");
            assert!(b.join("manifest.json").exists(), "{b:?} lost its manifest");
        }

        // `munich_plane` and not `munich_nodata`, `munich_nosub`, `munich_ref`
        // or `munich_mixed`. Each of the five is the lab as it stood, and the
        // reference has to be the lab as it stands: `munich_mixed` predates the
        // roof vote and the fusion response gate, `munich_ref` the registration
        // fix, `munich_nosub` the no-data roofline rule and `munich_nodata` the
        // exact plane fit. Two of those five changes were the same defect, a
        // verdict taken from a random sample of a point set: `register.py` used
        // to search a seeded random 4000 points of a pano's facade band and 6000
        // of a cluster's, and `plane.py` used to draw 300 two-point hypotheses by
        // index into an array `cKDTree` had ordered. Both now use every point,
        // and against `munich_plane` this run's registration is identical on all
        // 1076 panos, shift for shift.
        let ours = read_export(&cold.export_dir);
        let theirs = read_export(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tools/facade_lab/out/munich_plane/arnis_facades"),
        );
        assert!(!theirs.is_empty(), "the reference export must be readable");

        let mut only_ours: Vec<&String> =
            ours.keys().filter(|k| !theirs.contains_key(*k)).collect();
        let mut only_theirs: Vec<&String> =
            theirs.keys().filter(|k| !ours.contains_key(*k)).collect();
        only_ours.sort();
        only_theirs.sort();

        // The four numbers the tolerances are stated in, per wall: the class
        // grid agreement, the worst band colour, and the texture's mean
        // absolute difference and valid-mask IoU. The rectangle is reported
        // beside them because a wall whose rectangle moved cannot agree on
        // anything else, and that is where the two runs actually differ.
        let mut tier_moves = Vec::new();
        let mut rows = Vec::new();
        let (mut cells_all, mut colour_all, mut mad_all, mut iou_all) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut same_rect = 0usize;
        let mut size_moved = 0usize;
        // Height is the one product a wall can get badly wrong while agreeing on
        // everything else, and it is what a lost registration moves first.
        let mut height_moved: Vec<String> = Vec::new();
        // Pooled over cells rather than over walls, because that is the figure
        // the port is judged on: a small wall and a large one weigh what they
        // cover, not one vote each.
        let (mut pooled_same, mut pooled_cells) = (0usize, 0usize);
        let (mut window_inter, mut window_union) = (0usize, 0usize);
        for (key, ours_wall) in &ours {
            let Some(their_wall) = theirs.get(key) else {
                continue;
            };
            if ours_wall.tier != their_wall.tier {
                tier_moves.push(format!("{key} {} -> {}", their_wall.tier, ours_wall.tier));
            }
            let d_s = (ours_wall.s_l - their_wall.s_l).abs();
            let d_h = (ours_wall.height_used_m - their_wall.height_used_m).abs();
            if d_h > 0.5 {
                height_moved.push(format!(
                    "{key} {:.2} -> {:.2}",
                    their_wall.height_used_m, ours_wall.height_used_m
                ));
            }
            if ours_wall.cols != their_wall.cols || ours_wall.rows != their_wall.rows {
                size_moved += 1;
                rows.push(format!(
                    "{key}	size {}x{} -> {}x{}	ds_l {d_s:+.2}	dh {d_h:+.2}",
                    their_wall.cols, their_wall.rows, ours_wall.cols, ours_wall.rows
                ));
                continue;
            }
            if d_s < 0.02 && d_h < 0.02 {
                same_rect += 1;
            }
            let n = ours_wall.cells.len();
            let same = (0..n)
                .filter(|&i| ours_wall.cells[i][3] == their_wall.cells[i][3])
                .count();
            let agree = same as f64 / n.max(1) as f64;
            pooled_same += same;
            pooled_cells += n;
            // Windows on their own, because they are the smallest class and the
            // one a reader looks at: a wall can agree on 90 per cent of its
            // cells while putting every window somewhere else.
            let win = |c: &[u8; 4]| c[3] == super::super::openings::CLS_WINDOW;
            for i in 0..n {
                let (a, b) = (win(&ours_wall.cells[i]), win(&their_wall.cells[i]));
                window_inter += usize::from(a && b);
                window_union += usize::from(a || b);
            }
            let (a_bands, b_bands) = (ours_wall.bands(), their_wall.bands());
            let mut d_colour: f64 = 0.0;
            for (a, b) in a_bands.iter().zip(&b_bands) {
                if let (Some(a), Some(b)) = (a, b) {
                    d_colour = d_colour.max(imgops::oklab_distance(
                        imgops::srgb_to_oklab(*a),
                        imgops::srgb_to_oklab(*b),
                    ));
                }
            }
            let (mad, iou) = match (&ours_wall.tex, &their_wall.tex) {
                (Some((aw, ah, a)), Some((bw, bh, b))) if aw == bw && ah == bh => {
                    let (mut sum, mut count) = (0.0f64, 0usize);
                    let (mut inter, mut union) = (0usize, 0usize);
                    for i in 0..a.len() {
                        let (va, vb) = (a[i][3] > 0, b[i][3] > 0);
                        if va && vb {
                            inter += 1;
                            for k in 0..3 {
                                sum += (f64::from(a[i][k]) - f64::from(b[i][k])).abs();
                            }
                            count += 3;
                        }
                        if va || vb {
                            union += 1;
                        }
                    }
                    (
                        if count == 0 { 0.0 } else { sum / count as f64 },
                        if union == 0 {
                            1.0
                        } else {
                            inter as f64 / union as f64
                        },
                    )
                }
                _ => (f64::NAN, f64::NAN),
            };
            cells_all.push(agree);
            colour_all.push(d_colour);
            if mad.is_finite() {
                mad_all.push(mad);
                iou_all.push(iou);
            }
            if agree < 0.97 || d_colour > 0.03 {
                rows.push(format!(
                    "{key}	{}/{}	cells {agree:.3}	band {d_colour:.4}	texMAD {mad:.1}	IoU {iou:.3}	ds_l {d_s:+.2}	dh {d_h:+.2}",
                    their_wall.tier, ours_wall.tier
                ));
            }
        }
        let stat = |v: &mut Vec<f64>| -> (f64, f64, f64) {
            v.sort_by(f64::total_cmp);
            if v.is_empty() {
                return (f64::NAN, f64::NAN, f64::NAN);
            }
            (v[0], v[v.len() / 2], v[v.len() - 1])
        };
        println!(
            "walls: {} ours, {} theirs, {} shared; only ours {only_ours:?}; only theirs {only_theirs:?}",
            ours.len(),
            theirs.len(),
            ours.keys().filter(|k| theirs.contains_key(*k)).count()
        );
        println!(
            "rectangle: {same_rect} identical, {size_moved} a different number of blocks, {} shifted",
            cells_all.len() - same_rect
        );
        println!("tier moves ({}): {tier_moves:?}", tier_moves.len());
        let n_cells = cells_all.len();
        let pass_cells = cells_all.iter().filter(|v| **v >= 0.97).count();
        let pass_colour = colour_all.iter().filter(|v| **v <= 0.03).count();
        let pooled = pooled_same as f64 / pooled_cells.max(1) as f64;
        let (lo, cells_mid, hi) = stat(&mut cells_all);
        println!("class grid agreement: min {lo:.3} median {cells_mid:.3} max {hi:.3}; {pass_cells}/{n_cells} at or above 0.97");
        println!("class grid pooled:    {pooled:.3} over {pooled_cells} cells");
        let window_iou = window_inter as f64 / window_union.max(1) as f64;
        println!("window cell IoU:      {window_iou:.3} over {window_union} cells");
        let (lo, mid, hi) = stat(&mut colour_all);
        println!("band colour OkLab:    min {lo:.4} median {mid:.4} max {hi:.4}; {pass_colour}/{n_cells} at or below 0.03");
        let n_tex = mad_all.len();
        let pass_mad = mad_all.iter().filter(|v| **v < 4.0).count();
        let (lo, mad_mid, mad_hi) = stat(&mut mad_all);
        println!("texture MAD:          min {lo:.2} median {mad_mid:.2} max {mad_hi:.2}; {pass_mad}/{n_tex} under 4");
        let (iou_lo, iou_mid, hi) = stat(&mut iou_all);
        println!("texture valid IoU:    min {iou_lo:.3} median {iou_mid:.3} max {hi:.3}");
        println!(
            "height over 0.5 m:    {} walls: {height_moved:?}",
            height_moved.len()
        );
        for row in &rows {
            println!("  {row}");
        }

        // ---- and now the part that used to print and never object
        //
        // `PORT_REQUIREMENTS.md` states the target as the same walls at the same
        // tiers, block grids identical on at least 97 per cent of cells and band
        // colours within 0.03 in OkLab. The first is an equality and is asserted
        // as one. The other two hold on most walls and cannot hold on all of
        // them, so they are asserted as counts, and each count is the measured
        // number with room for the drift a compiler or an `image` release can
        // move a rounding by. What every bound below is really guarding is that
        // the two runs are looking at the same photographs at the same
        // resolution: starve a wall of imagery and `only_theirs`, `tier_moves`
        // and `pass_cells` all move at once, and read a 2048 px thumbnail where
        // the reference read a 5760 px original and `rect_moved`, `pass_mad`
        // and the window IoU move together.
        //
        // The differences that remain are known and bounded rather than open:
        //
        // * the plane fit used to be the biggest of them, because `plane.py`'s
        //   RANSAC drew point *indices* from a seeded numpy generator into an
        //   array whose order it could not reproduce at all: `cluster.near`
        //   hands back the cloud in the order `scipy.spatial.cKDTree` traverses
        //   it, and this port's clouds are ordered by OpenSfM point id. Same
        //   seed, same indices, different points. It is closed: the search is
        //   exact and hangs off the wall's own direction, so the two sides fit
        //   the same plane from the same point *set* and
        //   `golden_plane_fits_match_the_python_run` compares them as an
        //   equality, backwards as well as forwards;
        // * `blocks.py` did not survive the port, and its `observed` mask
        //   counted a cell's partially off-image texels where `class != no data`
        //   does not: 212 of 12276 fixture cells over 4 of 45 walls, per the
        //   module header;
        // * a wall whose rectangle moved cannot agree cell by cell on anything,
        //   so one such wall fails both per-wall tolerances at once.
        //
        // The rectangle bound is still the loosest of them and is the honest
        // state of the port rather than the target: 47 of the 111 walls land on
        // a different rectangle, 18 of them on a different number of blocks, and
        // until that is closed the class grid and band colour counts cannot
        // reach the 97 per cent and 0.03 that `PORT_REQUIREMENTS.md` asks for.
        // It is not one line of the table either, it is the cause under most of
        // the rest: split the walls where this run picks the same number of
        // blocks by how far the rectangle then shifted and the class agreement
        // follows the shift and nothing else. It was 77 before the plane fit
        // stopped depending on the order its points arrived in, and the exact
        // fit is what took it to 47.
        //
        // Every one of these is a ceiling or a floor, so a fix that improves the
        // run only leaves them slack; tighten them onto the new numbers when it
        // does.
        //
        // Measured (2026-09-05), then given about five per cent of room, which
        // is all a deterministic pipeline needs: the seeds are fixed, the
        // `rayon` loops are order independent, and two runs of this test print
        // the same table to the last cell. The room is for a compiler or an
        // `image` release moving a rounding, not for the run wandering.
        //
        // The first two columns are the two defects the numbers were once blind
        // to, measured by putting each one back. The last four are this run
        // against four references: `munich_ref` (the lab before the registration
        // fix, which is what the port was measured against while the fix was
        // found), `munich_nosub` (after it), `munich_nodata` (after the no-data
        // roofline rule as well) and `munich_plane` (after the exact plane fit,
        // which is what this test compares against).
        //
        // The `munich_nosub` column is the registration fix and nothing else.
        // The port's own code moved by two constants, and every cell of it
        // improves at once, because a pano that keeps its registration measures
        // its wall from where the cloud says the camera was rather than from the
        // raw Graph pose a median 5.15 m away. Walls whose height is more than
        // 0.5 m from the reference's fall from 19 to 10, and the two the port
        // was caught on, `w79817227_4` (12.00 against 19.75) and `w81190199_0`
        // (17.12 against 25.04), are both inside 0.5 m of it.
        //
        // The `munich_nodata` column is the no-data roofline rule on both sides
        // at once, so it moves less. What it takes off the table is
        // `w81190185_2`: the port read the boundary between the visible masonry
        // and the crop's own no-data as a roofline at 11.52 m on a building
        // tagged 21 m, and both sides now answer 21.00 m, which is why the
        // height list is one shorter.
        //
        // | | gate on the raw poses | no originals | vs munich_ref | vs munich_nosub | vs munich_nodata | vs munich_plane |
        // | walls the reference has and this run does not | 6 | 0 | 0 | 0 | 0 | 0 |
        // | walls on a different rectangle | 87 | 98 | 83 | 77 | 77 | **47** |
        // | walls agreeing on 97 % of classes | 8/58 | 3/61 | 10/69 | 13/77 | 13/75 | **28/93** |
        // | median wall's class agreement | 0.833 | 0.795 | 0.819 | 0.864 | 0.864 | **0.915** |
        // | pooled class agreement | 0.797 | 0.773 | 0.793 | 0.840 | 0.842 | **0.887** |
        // | window cell IoU | 0.390 | 0.408 | 0.397 | 0.505 | 0.508 | **0.625** |
        // | walls within 0.03 OkLab | 23/58 | 24/61 | 29/69 | 42/77 | 41/75 | **52/93** |
        // | median band colour OkLab | | | 0.0601 | 0.0188 | 0.0188 | **0.0147** |
        // | median texture MAD | 12.32 | 16.28 | 11.74 | 10.97 | 10.93 | **3.58** |
        // | worst texture MAD | 53.76 | 57.25 | 53.76 | 46.54 | 46.54 | 52.49 |
        // | walls more than 0.5 m apart in height | | | 19 | 10 | 9 | **3** |
        // | walls on a different tier | | | | | 8 | **6** |
        //
        // The `munich_plane` column is the exact plane fit on both sides at
        // once, and it is the largest step any of these has taken: the two runs
        // now fit the same plane from the same points, so a wall's rectangle
        // agrees unless the extent search on the port's own crop disagrees, and
        // the walls that keep the same number of blocks go from 75 to 93. Every
        // number improves except the worst texture MAD, and that one is the
        // comparison widening rather than a wall getting worse: the 18 extra
        // walls bring their own disagreements into the maximum, and the wall
        // holding it, `w81190178_0`, agrees on the rectangle to the block
        // (26 x 21, ends within a millimetre, height within 0.14 m) and
        // disagrees on what is inside it, marking 16.3 per cent of its cells
        // unknown against the reference's 3.8 on three views all more than 21 m
        // away. That is the occlusion planes, not the plane fit.
        //
        // The first column trips `only_theirs`, the second trips `rect_moved`,
        // and neither could trip anything before, which is why they both lived
        // here long enough to be measured in a generated world instead.
        //
        // In a world the difference is larger than the per wall numbers above,
        // and the reason is worth writing down because it is not a second
        // defect. Generating the Munich box three times from one binary, once
        // with no facades and once from each export (`--mapillary-facades-dir`,
        // scale 2, the same OSM file), the facades touch 421 476 cells and the
        // two exports disagree on 151 515 of them, 35.9 per cent. Measured the
        // same way against `munich_ref`, before the exact plane fit and before
        // the world edge fix, it was 49.9 per cent of 384 057 touched cells: the
        // disagreement fell while the covered area grew, the edge fix having
        // taken the matched buildings from 25 to 39 and the wall columns from
        // 998 to 1300.
        //
        // The gap between 11.3 per cent of classes and 35.9 per cent of blocks
        // is the colour quantisation rather than the pipeline. A cell's block is
        // the nearest palette entry to its colour, so a band only 0.0147 OkLab
        // away can still land on the neighbouring grey. Of the differing cells
        // whose two blocks are both in the palette, 30.9 per cent are a swap
        // inside 0.03 OkLab (diorite for polished diorite, diorite for smooth
        // stone), 45.3 per cent are inside 0.06, and 10.9 per cent are more than
        // 0.12 apart and read as a different colour.
        const ONLY_OURS_MAX: usize = 2;
        const TIER_MOVES_MAX: usize = 8;
        const RECT_MOVED_MAX: usize = 50;
        const CELLS_PASS_MIN: usize = 26;
        const CELLS_MEDIAN_MIN: f64 = 0.87;
        const CELLS_POOLED_MIN: f64 = 0.84;
        const WINDOW_IOU_MIN: f64 = 0.59;
        const COLOUR_PASS_MIN: usize = 49;
        const TEX_MAD_MEDIAN_MAX: f64 = 3.8;
        const TEX_MAD_MAX: f64 = 55.0;
        const TEX_IOU_MEDIAN_MIN: f64 = 0.99;
        const TEX_IOU_MIN: f64 = 0.80;
        const HEIGHT_MOVED_MAX: usize = 5;
        const ORIGINALS_MIN: usize = 200;
        assert!(
            only_theirs.is_empty(),
            "the reference textured {} walls this run did not: {only_theirs:?}",
            only_theirs.len()
        );
        assert!(
            only_ours.len() <= ONLY_OURS_MAX,
            "{} walls textured here and not there (at most {ONLY_OURS_MAX}): {only_ours:?}",
            only_ours.len()
        );
        assert!(
            tier_moves.len() <= TIER_MOVES_MAX,
            "{} walls changed tier (at most {TIER_MOVES_MAX}): {tier_moves:?}",
            tier_moves.len()
        );
        let rect_moved = (n_cells - same_rect) + size_moved;
        assert!(
            rect_moved <= RECT_MOVED_MAX,
            "{rect_moved} walls landed on a different rectangle (at most {RECT_MOVED_MAX})"
        );
        assert!(
            pass_cells >= CELLS_PASS_MIN,
            "{pass_cells}/{n_cells} walls agree on 97 per cent of their block classes (at least {CELLS_PASS_MIN})"
        );
        assert!(
            cells_mid >= CELLS_MEDIAN_MIN,
            "the median wall agrees on {cells_mid:.3} of its block classes (at least {CELLS_MEDIAN_MIN})"
        );
        assert!(
            pooled >= CELLS_POOLED_MIN,
            "{pooled:.3} of all {pooled_cells} compared cells carry the same class (at least {CELLS_POOLED_MIN})"
        );
        assert!(
            window_iou >= WINDOW_IOU_MIN,
            "the two runs put their windows on the same cell {window_iou:.3} of the time (at least {WINDOW_IOU_MIN})"
        );
        assert!(
            pass_colour >= COLOUR_PASS_MIN,
            "{pass_colour}/{n_cells} walls keep every band colour within 0.03 OkLab (at least {COLOUR_PASS_MIN})"
        );
        assert!(
            mad_mid <= TEX_MAD_MEDIAN_MAX,
            "the median wall's texture differs by {mad_mid:.2} sRGB units (at most {TEX_MAD_MEDIAN_MAX})"
        );
        assert!(
            mad_hi <= TEX_MAD_MAX,
            "the worst wall's texture differs by {mad_hi:.2} sRGB units (at most {TEX_MAD_MAX})"
        );
        assert!(
            iou_mid >= TEX_IOU_MEDIAN_MIN,
            "the median wall's texture covers an IoU of {iou_mid:.3} (at least {TEX_IOU_MEDIAN_MIN})"
        );
        assert!(
            iou_lo >= TEX_IOU_MIN,
            "the worst wall's texture covers an IoU of {iou_lo:.3} (at least {TEX_IOU_MIN})"
        );
        assert!(
            height_moved.len() <= HEIGHT_MOVED_MAX,
            "{} walls are more than 0.5 m from the reference in height (at most              {HEIGHT_MOVED_MAX}): {height_moved:?}",
            height_moved.len()
        );

        // The two downloads, which are the defects the numbers above were blind
        // to. The thumbnails are a real subset of the box, so the gate still
        // scopes; the originals are fetched at all, which is what the `hires`
        // ladder needs to mean anything outside a replay; and a second run over
        // the same area fetches none of them, because every wall is cached.
        assert!(
            cold.stats.images_downloaded < cold.stats.images,
            "the gate downloaded all {} images of the box",
            cold.stats.images
        );
        assert!(
            cold.stats.originals_downloaded >= ORIGINALS_MIN,
            "the texture stage read {} originals (at least {ORIGINALS_MIN})",
            cold.stats.originals_downloaded
        );
        assert_eq!(
            warm.stats.originals_downloaded, 0,
            "a generation over an area already built must fetch no originals"
        );
    }
}
