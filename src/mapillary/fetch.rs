//! Downloading metadata, clusters and images. Port of `tools/facade_lab/prefetch.py`.
//!
//! Three things the network makes this stage carry, all of them measured on
//! real runs rather than guessed:
//!
//! * A `bbox` search must be under 0.01 degrees, and a dense cell answers HTTP
//!   500 "reduce the amount of data" even inside that limit, so the tiler
//!   subdivides the cell into quadrants and retries. Five levels take an 890 m
//!   cell down to 28 m, which is where the Python prototype stops, and a cell
//!   still refused there is a hole in coverage that no further splitting fixes.
//!   The grid and the quadrant split are `api.rs`'s, which already had them.
//! * A signed CDN URL expires after about 30 days, so the useful cache is the
//!   bytes and not the URLs. A 403 or 404 on one means it expired: the image is
//!   re-queried once for fresh URLs and retried, and never a second time.
//! * The socket timeout only fires when no bytes arrive at all. A throttled CDN
//!   that trickles a few KB a minute never trips it, and one such download
//!   stalled a whole New York fetch for half an hour. Bodies are therefore read
//!   chunk by chunk against a total deadline, and reqwest's blocking `Read`
//!   restarts its timeout on every call, so nothing else was going to catch it.
//!
//! What is requested per image: the pose fields the geometry stage needs
//! (`computed_geometry`, `computed_rotation`, `computed_compass_angle`,
//! `computed_altitude`, with the raw `geometry`, `compass_angle` and `altitude`
//! as fallbacks), the camera model (`camera_type`, `camera_parameters`), the
//! cluster (`sfm_cluster`), the selection fields (`captured_at`, `sequence`,
//! `quality_score`, `atomic_scale`, `width`, `height`, `is_pano`), the three
//! thumbnail URLs, and `creator` for the credits.
//!
//! **There is no title field.** The Graph API image entity has `creator`
//! (`{username, id}`, present on all 398 images of the Munich box) but nothing
//! carrying a title, caption, description, place or locality: asking for any of
//! them answers `MLYApiException` code 100, "Tried accessing nonexisting
//! field". `organization` exists but answers `{id}` alone, has no nested field
//! selection, and covers 84 of those 398, so it is not a title either. The
//! credit line therefore takes its title from [`FetchConfig::area_label`], which
//! the caller fills with the world's place name (`retrieve_data::fetch_area_name`
//! already computes one for the world title), and falls back to naming the
//! image. Mapillary's own example, `[Madeira, Portugal] by [nunocaldeira]`, is
//! a place name too.
//!
//! The OSM half of `prefetch.py` is [`fetch_osm`]: a buildings-only Overpass
//! query over the wider margin, because a building outside the box can still
//! occlude one inside it. It is deliberately separate from [`fetch_metadata`],
//! since a caller that already has Arnis's own parsed elements does not need it
//! and an Overpass outage must not cost the much slower imagery fetch.
//!
//! The cache layout and everything on-disk is [`super::cache`].

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde_json::Value;

use crate::coordinate_system::geographic::LLBBox;
use crate::net::request_permit;
use crate::progress::{emit_gui_progress_update, MESSAGE_ONLY};

use super::api;
use super::cache::{self, ImageSize, Layout};
use super::types::{BBox, CameraModel, PanoMeta};

/// Every field the pipeline reads, plus the two the credits need.
const FIELDS: &str = "id,computed_geometry,geometry,computed_compass_angle,compass_angle,\
computed_rotation,computed_altitude,altitude,atomic_scale,camera_type,is_pano,camera_parameters,\
quality_score,width,height,captured_at,sequence,creator,\
thumb_1024_url,thumb_2048_url,thumb_original_url,sfm_cluster";

/// What a re-query after a 403 asks for: only the signed URLs go stale.
const URL_FIELDS: &str = "thumb_original_url,thumb_2048_url,thumb_1024_url,sfm_cluster";

/// Total budget for one download, from the connect to the last byte. The
/// Python uses the same four minutes; an honest 2048 px thumbnail is 300 KB and
/// arrives in under a second, so this only ever fires on a stall.
pub const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(240);

/// How long a batch of downloads may deliver nothing at all before it is
/// abandoned. Not a budget for the batch: a city on a slow line is an honest
/// hour of downloading and every file that lands resets this. Five minutes is
/// longer than [`DOWNLOAD_DEADLINE`] takes to give up on one stalled file, so it
/// only fires when every worker is stuck at once, and a batch that is plainly
/// failing is ended by [`BATCH_GIVE_UP_FAILURES`] long before it.
pub const BATCH_DEADLINE: Duration = Duration::from_secs(300);

/// Per read call, matching the Python's `timeout=(20, 60)`. Not a total: see
/// the module header.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Refuse a body larger than this. An original equirectangular JPEG is about
/// 5 MB and a cluster about 3 MB compressed, so this is a guard against a
/// misdirected URL, not a real limit.
const MAX_BODY_BYTES: usize = 96 * 1024 * 1024;

const CHUNK_BYTES: usize = 64 * 1024;

/// Downloads in flight. Well under the global 16 of `net::request_permit`, so
/// imagery never crowds out the elevation and OSM fetches it runs beside.
const DEFAULT_PARALLEL: usize = 6;

/// Overpass mirrors, the Arnis one first. Same list and same order as the rest
/// of Arnis uses, so its server sees one client.
const OVERPASS_MIRRORS: [&str; 5] = [
    "https://api.arnismc.com/overpass/api/interpreter",
    "https://overpass-api.de/api/interpreter",
    "https://lz4.overpass-api.de/api/interpreter",
    "https://z.overpass-api.de/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
];

// --------------------------------------------------------------------------- configuration

/// Where the Graph API lives. A field rather than a constant only so the tests
/// can point it at a local server; nothing else ever changes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// The image search.
    pub images: String,
    /// The single image entity; `{id}` is appended.
    pub image: String,
    pub overpass: Vec<String>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            images: "https://graph.mapillary.com/images".to_string(),
            image: "https://graph.mapillary.com".to_string(),
            overpass: OVERPASS_MIRRORS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// How hard to try before giving up on a request or a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// Attempts per request, counting the first.
    pub attempts: u32,
    /// First backoff step; it doubles per attempt and a `Retry-After` header
    /// wins over it.
    pub backoff: Duration,
    /// How many times a refused search cell may be quartered.
    pub subdivision_depth: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            attempts: 5,
            backoff: Duration::from_secs(5),
            subdivision_depth: api::MAX_SUBDIVISION_DEPTH,
        }
    }
}

/// What a run needs before it can fetch anything.
#[derive(Clone, Debug)]
pub struct FetchConfig {
    pub token: String,
    /// The world's bbox, before either margin is applied.
    pub bbox: BBox,
    pub pano_margin_m: f64,
    pub osm_margin_m: f64,
    /// Camera classes to keep. Empty means every class the API returns.
    pub camera_types: Vec<CameraModel>,
    /// Total budget for one download, not for the batch.
    pub deadline: Duration,
    /// How long a batch may deliver nothing before it is abandoned, measured
    /// from its last delivery rather than from its start. The consecutive
    /// failure rule in [`BatchGuard`] is what usually ends a dead batch, in
    /// seconds; this is the backstop for one that hangs instead of failing.
    pub batch_deadline: Duration,
    pub cache_dir: PathBuf,
    /// The pipeline runs on 2048 everywhere; the original is a detail mode.
    pub size: ImageSize,
    pub parallel: usize,
    /// The place name to credit imagery under, because the Graph API has no
    /// title field. See the module header.
    pub area_label: Option<String>,
    pub endpoints: Endpoints,
    pub limits: Limits,
}

impl FetchConfig {
    pub fn new(token: impl Into<String>, bbox: BBox) -> Self {
        Self {
            token: token.into(),
            bbox,
            pano_margin_m: 45.0,
            osm_margin_m: 60.0,
            camera_types: Vec::new(),
            deadline: DOWNLOAD_DEADLINE,
            batch_deadline: BATCH_DEADLINE,
            cache_dir: cache::root(),
            size: ImageSize::W2048,
            parallel: DEFAULT_PARALLEL,
            area_label: None,
            endpoints: Endpoints::default(),
            limits: Limits::default(),
        }
    }

    pub fn layout(&self) -> Layout {
        Layout::new(self.cache_dir.clone())
    }

    /// True when this run keeps images of that camera class.
    fn admits(&self, model: CameraModel) -> bool {
        self.camera_types.is_empty() || self.camera_types.contains(&model)
    }
}

/// The attribution one image carries, kept with its metadata so a run served
/// entirely from cache can still credit it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    pub pano_id: String,
    pub title: String,
    pub creator: String,
    pub creator_url: String,
    pub image_url: String,
}

/// Which image owns a cluster, so its signed URL can be found again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterRef {
    pub id: String,
    /// One image of the cluster; several share a cluster and any of them can
    /// hand out its URL.
    pub pano_id: String,
}

/// Everything the metadata pass brings back.
#[derive(Clone, Debug, Default)]
pub struct Fetched {
    pub metas: Vec<PanoMeta>,
    pub credits: Vec<Credit>,
    pub clusters: Vec<ClusterRef>,
    /// The raw Overpass answer, parsed by `geometry::parse_overpass`. Null
    /// until [`fetch_osm`] fills it.
    pub osm: Value,
    /// Search cells the API refused even at the smallest size, which is a hole
    /// in coverage rather than a failure.
    pub cells_refused: usize,
}

// --------------------------------------------------------------------------- HTTP

enum HttpError {
    /// 403 or 404 on a signed URL: it expired and must be re-queried once.
    Forbidden,
    /// The Graph 500 that asks for a smaller cell.
    TooLarge,
    Failed(String),
}

impl HttpError {
    fn message(&self) -> String {
        match self {
            HttpError::Forbidden => "signed URL expired".to_string(),
            HttpError::TooLarge => "response too large".to_string(),
            HttpError::Failed(e) => e.clone(),
        }
    }
}

/// One reply, read to the end.
struct Reply {
    status: reqwest::StatusCode,
    retry_after_s: Option<f64>,
    body: Vec<u8>,
}

enum Attempt {
    Got(Reply),
    /// Nothing usable came back: the connection was refused, timed out, or the
    /// body arrived too slowly to be worth waiting for.
    ///
    /// One case, not three. The retry budget exists for a server that answered
    /// something, and a host that will not talk to us answers the same way
    /// however it declines: a refused connection used to buy the full five
    /// attempts and four growing backoffs, fifty seconds of sleeping per file,
    /// which on a dead image CDN is what left a generation still retrying after
    /// 507 seconds with the world waiting on it.
    Undelivered(String),
}

struct Http {
    client: reqwest::blocking::Client,
    limits: Limits,
    deadline: Duration,
}

impl Http {
    fn new(cfg: &FetchConfig) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(READ_TIMEOUT)
            .user_agent(crate::retrieve_data::OSM_USER_AGENT)
            .build()
            .map_err(|e| format!("Mapillary HTTP client: {e}"))?;
        Ok(Self {
            client,
            limits: cfg.limits,
            deadline: cfg.deadline,
        })
    }

    fn attempt(&self, url: &str, query: &[(&str, &str)]) -> Attempt {
        let _permit = request_permit();
        let mut response = match self.client.get(url).query(query).send() {
            Ok(r) => r,
            // without_url: the request URL carries `access_token=MLY|...` as a
            // query parameter, and reqwest's Display prints the URL, so the
            // token would reach stderr and the GUI status line on any transport
            // failure.
            Err(e) => return Attempt::Undelivered(e.without_url().to_string()),
        };
        let status = response.status();
        let retry_after_s = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok());
        match read_with_deadline(&mut response, self.deadline) {
            Ok(body) => Attempt::Got(Reply {
                status,
                retry_after_s,
                body,
            }),
            Err(e) => Attempt::Undelivered(e),
        }
    }

    /// GET with the retry rules of `prefetch.get`.
    ///
    /// `graph` marks a Graph API call, whose 500 means "ask for less" whatever
    /// the wording; a download's 500 is just a 500.
    fn get(&self, url: &str, query: &[(&str, &str)], graph: bool) -> Result<Vec<u8>, HttpError> {
        let mut last = String::new();
        let mut undelivered = 0u32;
        let attempts = self.limits.attempts.max(1);

        for attempt in 0..attempts {
            let mut wait = self.limits.backoff * (attempt + 1);
            match self.attempt(url, query) {
                Attempt::Got(reply) => {
                    if reply.status.is_success() {
                        return Ok(reply.body);
                    }
                    let text = String::from_utf8_lossy(&reply.body);
                    // Authentication first, because the two questions below
                    // it answer yes to shapes a refused token also arrives in:
                    // any Graph 500 counts as "ask for less" whatever it says,
                    // so a refusal carrying one would be read as a dense cell
                    // and answered by subdividing it, once per sub-cell, until
                    // the area was reported as a hole in coverage rather than
                    // as the bad token it is.
                    if api::is_auth_failure(reply.status, &text) {
                        // A token the API refuses will be refused again, so
                        // retrying it only makes the user wait for the same
                        // answer once per attempt and once per search cell.
                        return Err(HttpError::Failed(format!(
                            "{} Check the Mapillary token.",
                            api::describe_failure(reply.status, &text)
                        )));
                    }
                    if reply.status.as_u16() == 500 && (graph || api::wants_smaller_area(&text)) {
                        return Err(HttpError::TooLarge);
                    }
                    if matches!(reply.status.as_u16(), 403 | 404) {
                        return Err(HttpError::Forbidden);
                    }
                    if reply.status.as_u16() == 429 {
                        // Honour Retry-After when it is a plain number of
                        // seconds, else back off exponentially; either way cap
                        // the wait so one rate limit cannot hang a generation.
                        let backoff = self.limits.backoff.as_secs_f64() * 2f64.powi(attempt as i32);
                        let seconds = reply.retry_after_s.unwrap_or(backoff);
                        wait = Duration::from_secs_f64(seconds.clamp(0.0, 120.0));
                        last = "rate limited (429)".to_string();
                    } else {
                        last = api::describe_failure(reply.status, &text);
                    }
                }
                Attempt::Undelivered(e) => {
                    // The first may be a hiccup; a second on the same URL is a
                    // host that is not going to deliver, whether it refused the
                    // connection, timed out or trickled.
                    undelivered += 1;
                    if undelivered >= 2 {
                        return Err(HttpError::Failed(e));
                    }
                    last = e;
                }
            }
            if attempt + 1 < attempts {
                std::thread::sleep(wait);
            }
        }
        Err(HttpError::Failed(last))
    }
}

/// Reads a body chunk by chunk against a total deadline.
fn read_with_deadline(
    response: &mut reqwest::blocking::Response,
    deadline: Duration,
) -> Result<Vec<u8>, String> {
    let start = Instant::now();
    let mut body = Vec::new();
    let mut chunk = vec![0u8; CHUNK_BYTES];
    loop {
        let read = response
            .read(&mut chunk)
            .map_err(|e| format!("read failed: {e}"))?;
        if read == 0 {
            return Ok(body);
        }
        body.extend_from_slice(&chunk[..read]);
        if body.len() > MAX_BODY_BYTES {
            return Err(format!("body over the {MAX_BODY_BYTES} byte cap"));
        }
        if start.elapsed() > deadline {
            return Err(format!(
                "download stalled ({} KB in {:.0} s)",
                body.len() / 1024,
                deadline.as_secs_f64()
            ));
        }
    }
}

// --------------------------------------------------------------------------- metadata

/// `(min_lat, min_lon, max_lat, max_lon)` grown by `margin_m` on every side.
pub fn pad_bbox(bbox: BBox, margin_m: f64) -> BBox {
    const M_PER_DEG_LAT: f64 = 111_195.0;
    let dlat = margin_m / M_PER_DEG_LAT;
    // Near the poles a metre is a lot of longitude; the clamp keeps the padded
    // box finite rather than correct, which is all any of this can be there.
    let cos_lat = (0.5 * (bbox.min_lat + bbox.max_lat))
        .to_radians()
        .cos()
        .abs()
        .max(1e-6);
    let dlon = margin_m / (M_PER_DEG_LAT * cos_lat);
    BBox::new(
        (bbox.min_lat - dlat).max(-89.9),
        (bbox.min_lon - dlon).max(-180.0),
        (bbox.max_lat + dlat).min(89.9),
        (bbox.max_lon + dlon).min(180.0),
    )
}

/// What one search cell produced, including the cells that were refused
/// outright so the caller can say how much of the area went unseen.
struct CellResult {
    records: Vec<Value>,
    refused: usize,
}

/// One search cell, quartered and retried while the API asks for less.
fn search_cell(
    http: &Http,
    cfg: &FetchConfig,
    cell: api::Cell,
    depth: u32,
) -> Result<CellResult, String> {
    let (min_lon, min_lat, max_lon, max_lat) = cell;
    let bbox = format!("{min_lon},{min_lat},{max_lon},{max_lat}");
    let query = [
        ("access_token", cfg.token.as_str()),
        ("fields", FIELDS),
        ("bbox", bbox.as_str()),
    ];

    match http.get(&cfg.endpoints.images, &query, true) {
        Ok(body) => {
            let parsed: Value = serde_json::from_slice(&body)
                .map_err(|e| format!("Mapillary response was not the expected JSON: {e}"))?;
            let records = match parsed.get("data") {
                Some(Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            Ok(CellResult {
                records,
                refused: 0,
            })
        }
        Err(HttpError::TooLarge) => {
            if depth >= cfg.limits.subdivision_depth {
                // A cell of a few tens of metres that is still refused cannot
                // be split into an answer. Count it and go on: the rest of the
                // area is worth more than this cell. Counted rather than
                // printed, because a search that is being refused for a reason
                // other than size is refused in every leaf, and a thousand
                // identical lines say no more than one.
                return Ok(CellResult {
                    records: Vec::new(),
                    refused: 1,
                });
            }
            let parts: Vec<Result<CellResult, String>> = api::quadrants(cell)
                .par_iter()
                .map(|&quadrant| search_cell(http, cfg, quadrant, depth + 1))
                .collect();
            let mut merged = CellResult {
                records: Vec::new(),
                refused: 0,
            };
            for part in parts {
                let part = part?;
                merged.records.extend(part.records);
                merged.refused += part.refused;
            }
            Ok(merged)
        }
        Err(e) => Err(format!("Mapillary search: {}", e.message())),
    }
}

/// Image metadata for the bbox plus its imagery margin.
///
/// Every record found is written to the cache, whatever its camera class, and
/// only the admitted classes come back: the metadata is cheap and an id keyed
/// entry is reusable by a later run with different camera settings.
pub fn fetch_metadata(cfg: &FetchConfig) -> Result<Fetched, String> {
    let http = Http::new(cfg)?;
    let padded = pad_bbox(cfg.bbox, cfg.pano_margin_m);
    let llbbox = LLBBox::new(
        padded.min_lat,
        padded.min_lon,
        padded.max_lat,
        padded.max_lon,
    )?;
    let cells = api::search_cells(&llbbox);

    emit_gui_progress_update(MESSAGE_ONLY, "Mapillary facades: searching coverage...");
    let results: Vec<Result<CellResult, String>> = cells
        .par_iter()
        .map(|&cell| search_cell(&http, cfg, cell, 0))
        .collect();

    // A single failed cell is a hole in coverage as long as others answered,
    // but if every cell failed then a bad token and genuinely uncovered ground
    // look identical, which is the most confusing way this can go wrong.
    let total = results.len();
    let mut records: BTreeMap<String, Value> = BTreeMap::new();
    let mut refused = 0usize;
    let mut first_error: Option<String> = None;
    let mut failed = 0usize;
    for result in results {
        match result {
            Ok(cell) => {
                refused += cell.refused;
                for record in cell.records {
                    if let Some(id) = record_id(&record) {
                        records.insert(id, record);
                    }
                }
            }
            Err(e) => {
                failed += 1;
                first_error.get_or_insert(e);
            }
        }
    }
    if let Some(error) = first_error {
        if failed == total {
            return Err(error);
        }
        eprintln!("Warning: {failed} of {total} Mapillary search cells failed ({error})");
    }
    // The size refusal is a 500 whose wording varies, so the tiler splits on
    // any Graph 500 (the Python does the same, and dense city cells need it).
    // That makes a refusal the API is issuing for some other reason look like a
    // very dense area, and it is only visible in the aggregate: an area really
    // too dense to answer still answers somewhere, while one refused down to
    // the smallest cell everywhere is being refused for a reason that has
    // nothing to do with size.
    if records.is_empty() && refused > 0 {
        return Err(format!(
            "Mapillary refused every one of the {refused} search cells for this area. \
             A token the API does not accept is the usual cause; a service outage is the other."
        ));
    }
    if refused > 0 {
        eprintln!(
            "Note: Mapillary could not answer {refused} search cells; that much of the area went unseen."
        );
    }

    let layout = cfg.layout();
    let mut metas = Vec::new();
    let mut credits = Vec::new();
    let mut clusters: Vec<ClusterRef> = Vec::new();
    let mut seen_clusters: BTreeSet<String> = BTreeSet::new();
    for (id, record) in &records {
        store_meta(&layout, id, record);
        let Some(meta) = PanoMeta::from_graph(record) else {
            continue;
        };
        if !cfg.admits(meta.camera_type) {
            continue;
        }
        if let Some(cluster_id) = meta.cluster_id.clone() {
            if seen_clusters.insert(cluster_id.clone()) {
                clusters.push(ClusterRef {
                    id: cluster_id,
                    pano_id: meta.id.clone(),
                });
            }
        }
        credits.push(credit_from(cfg, record, &meta.id));
        metas.push(meta);
    }

    Ok(Fetched {
        metas,
        credits,
        clusters,
        osm: Value::Null,
        cells_refused: refused,
    })
}

/// The id of a Graph record, whether it came back as a string or a number.
fn record_id(record: &Value) -> Option<String> {
    match record.get("id")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The credit for one image. See the module header on why the title is not a
/// field of the record.
fn credit_from(cfg: &FetchConfig, record: &Value, id: &str) -> Credit {
    let creator = record.get("creator");
    let username = creator
        .and_then(|c| c.get("username"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let user_id = creator
        .and_then(|c| c.get("id"))
        .map(|v| match v.as_str() {
            Some(s) => s.to_string(),
            None => v.to_string(),
        })
        .unwrap_or_default();
    let handle = if username.is_empty() {
        user_id.clone()
    } else {
        username.clone()
    };
    Credit {
        pano_id: id.to_string(),
        title: match cfg.area_label.as_deref().map(str::trim) {
            Some(label) if !label.is_empty() => label.to_string(),
            _ => format!("Mapillary image {id}"),
        },
        creator: username,
        creator_url: format!("https://www.mapillary.com/app/user/{handle}"),
        image_url: format!("https://www.mapillary.com/app/?pKey={id}&focus=photo"),
    }
}

/// Writes one Graph record into the metadata cache, merging it over whatever is
/// already there so a re-query that asked for the URL fields alone cannot lose
/// the pose fields.
fn store_meta(layout: &Layout, id: &str, record: &Value) {
    let Ok(path) = layout.meta_path(id) else {
        return;
    };
    let mut merged = load_meta(layout, id).unwrap_or_else(|| Value::Object(Default::default()));
    match (merged.as_object_mut(), record.as_object()) {
        (Some(target), Some(source)) => {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        _ => merged = record.clone(),
    }
    if let Ok(bytes) = serde_json::to_vec(&merged) {
        if let Err(e) = cache::write_atomic(&path, &bytes) {
            eprintln!("Note: Mapillary metadata for {id} not cached: {e}");
        }
    }
}

/// One cached Graph record.
pub fn load_meta(layout: &Layout, id: &str) -> Option<Value> {
    let path = layout.meta_path(id).ok()?;
    serde_json::from_slice(&cache::read_cached(&path)?).ok()
}

/// Fresh signed URLs for one image, merged into its cached record.
fn requery(http: &Http, cfg: &FetchConfig, layout: &Layout, id: &str) -> Result<Value, String> {
    let url = format!("{}/{}", cfg.endpoints.image.trim_end_matches('/'), id);
    let query = [("access_token", cfg.token.as_str()), ("fields", URL_FIELDS)];
    let body = http
        .get(&url, &query, true)
        .map_err(|e| format!("re-query {id}: {}", e.message()))?;
    let fresh: Value =
        serde_json::from_slice(&body).map_err(|e| format!("re-query {id}: bad JSON: {e}"))?;
    store_meta(layout, id, &fresh);
    load_meta(layout, id).ok_or_else(|| format!("re-query {id}: nothing cached"))
}

/// The signed URL for one thumbnail size out of a record.
fn thumb_url(record: &Value, size: ImageSize) -> Option<String> {
    record
        .get(size.url_field())
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

fn cluster_url(record: &Value) -> Option<String> {
    record
        .get("sfm_cluster")
        .and_then(|c| c.get("url"))
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

// --------------------------------------------------------------------------- images

/// What a download batch achieved. Paths and not bytes: a wall reads its own
/// views when it needs them, so a city's imagery never sits in memory at once.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub ready: BTreeMap<String, PathBuf>,
    pub failed: Vec<(String, String)>,
}

impl Batch {
    pub fn path(&self, id: &str) -> Option<&PathBuf> {
        self.ready.get(id)
    }
}

/// When a batch stops trying.
///
/// The imagery download sits in front of the buildings: nothing in the world is
/// built until it is done, so a batch that cannot finish has to fail rather than
/// keep the generation waiting. Two ways out, both about the batch and not about
/// one file, because a host that is down is down for all of them, and both
/// counted **since the last delivery** rather than from the start. That is the
/// difference between "this batch has stopped working" and "this batch is
/// taking a while": a city on a slow line is hours of honest downloading and
/// must not be cut off, and a file found in the cache counts as a delivery too.
struct BatchGuard {
    started: Instant,
    /// How long the batch may go without delivering anything.
    silence: Duration,
    /// Milliseconds since `started` at the last delivery.
    last_ok_ms: AtomicU64,
    /// Failures since the last delivery. A host that is answering resets it, so
    /// a slow link or a handful of missing images never trips it.
    since_ok: AtomicUsize,
    give_up_after: usize,
}

/// Consecutive failures, with nothing delivered in between, that say the far end
/// is not going to deliver anything.
///
/// Two full waves of [`DEFAULT_PARALLEL`], so a batch has to have every worker
/// fail twice over before it is written off. A real batch loses the odd image to
/// a 404 between successes and never comes near it; an unreachable host reaches
/// it in the time of two connect attempts and the rest of the batch then costs
/// nothing at all.
const BATCH_GIVE_UP_FAILURES: usize = 2 * DEFAULT_PARALLEL;

impl BatchGuard {
    fn new(cfg: &FetchConfig) -> Self {
        Self {
            started: Instant::now(),
            silence: cfg.batch_deadline,
            last_ok_ms: AtomicU64::new(0),
            since_ok: AtomicUsize::new(0),
            give_up_after: BATCH_GIVE_UP_FAILURES,
        }
    }

    /// Why this batch has stopped, or `None` while it may carry on.
    fn stopped(&self) -> Option<String> {
        if self.since_ok.load(Ordering::Relaxed) >= self.give_up_after {
            return Some(format!(
                "gave up after {} downloads in a row failed",
                self.give_up_after
            ));
        }
        // Saturating: another thread can record a delivery between the two
        // reads, which would otherwise wrap round to an enormous silence.
        let quiet = (self.started.elapsed().as_millis() as u64)
            .saturating_sub(self.last_ok_ms.load(Ordering::Relaxed));
        if quiet > self.silence.as_millis() as u64 {
            return Some(format!(
                "the download batch delivered nothing for {:.0} s",
                self.silence.as_secs_f64()
            ));
        }
        None
    }

    fn record(&self, ok: bool) {
        if ok {
            self.last_ok_ms
                .store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
            self.since_ok.store(0, Ordering::Relaxed);
        } else {
            self.since_ok.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Downloads every image that is not already cached, in parallel.
///
/// One failure is one missing view, never a failed batch: a wall with three
/// candidate views loses one of them and carries on, which is exactly what the
/// stall guard exists for.
pub fn download_images(cfg: &FetchConfig, ids: &[String]) -> Batch {
    // A generation over an area already built asks for no originals at all, and
    // must not announce a download it is not going to make.
    if ids.is_empty() {
        return Batch::default();
    }
    let http = match Http::new(cfg) {
        Ok(h) => h,
        Err(e) => {
            return Batch {
                ready: BTreeMap::new(),
                failed: ids.iter().map(|id| (id.clone(), e.clone())).collect(),
            }
        }
    };
    let layout = cfg.layout();
    let done = AtomicUsize::new(0);
    let total = ids.len();
    emit_gui_progress_update(MESSAGE_ONLY, "Mapillary facades: downloading imagery...");

    let guard = BatchGuard::new(cfg);
    let results: Vec<(String, Result<PathBuf, String>)> = in_pool(cfg.parallel, || {
        ids.par_iter()
            .map(|id| {
                let outcome = ensure_image(&http, cfg, &layout, id, &guard);
                report(&done, total, "Downloading Mapillary imagery");
                (id.clone(), outcome)
            })
            .collect()
    });

    let mut batch = Batch::default();
    for (id, outcome) in results {
        match outcome {
            Ok(path) => {
                batch.ready.insert(id, path);
            }
            Err(e) => batch.failed.push((id, e)),
        }
    }
    batch
}

/// The cached file for one image, downloading it if it is not there yet.
fn ensure_image(
    http: &Http,
    cfg: &FetchConfig,
    layout: &Layout,
    id: &str,
    guard: &BatchGuard,
) -> Result<PathBuf, String> {
    let path = layout.image_path(id, cfg.size)?;
    if cache::read_cached(&path).is_some() {
        // Before the guard, always. A batch that has given up on the network
        // must still hand back everything already on disk, or a run that lost
        // its last few downloads would lose the hundreds it already had.
        guard.record(true);
        return Ok(path);
    }
    if let Some(why) = guard.stopped() {
        return Err(format!("{id}: {why}"));
    }
    let outcome = download_image(http, cfg, layout, id, &path);
    guard.record(outcome.is_ok());
    outcome
}

/// One image off the network and into the cache.
fn download_image(
    http: &Http,
    cfg: &FetchConfig,
    layout: &Layout,
    id: &str,
    path: &Path,
) -> Result<PathBuf, String> {
    // One re-query per image, whether it was spent on a missing URL or on a
    // 403: a URL that is still refused after a fresh one is not an expiry.
    let mut requeried = false;
    let mut record = match load_meta(layout, id) {
        Some(record) => record,
        None => {
            requeried = true;
            requery(http, cfg, layout, id)?
        }
    };
    let mut url = match thumb_url(&record, cfg.size) {
        Some(url) => url,
        None if !requeried => {
            requeried = true;
            record = requery(http, cfg, layout, id)?;
            thumb_url(&record, cfg.size)
                .ok_or_else(|| format!("{id}: no {} thumbnail URL", cfg.size.suffix()))?
        }
        None => return Err(format!("{id}: no {} thumbnail URL", cfg.size.suffix())),
    };

    loop {
        match http.get(&url, &[], false) {
            Ok(bytes) => {
                if !looks_like_jpeg(&bytes) {
                    return Err(format!("{id}: {} bytes that are not a JPEG", bytes.len()));
                }
                cache::write_atomic(path, &bytes)?;
                return Ok(path.to_path_buf());
            }
            Err(HttpError::Forbidden) if !requeried => {
                requeried = true;
                record = requery(http, cfg, layout, id)?;
                url = thumb_url(&record, cfg.size).ok_or_else(|| {
                    format!(
                        "{id}: no {} thumbnail URL after re-query",
                        cfg.size.suffix()
                    )
                })?;
            }
            Err(e) => return Err(format!("{id}: {}", e.message())),
        }
    }
}

/// A JPEG starts `FF D8 FF`. Cheap enough to run on every download, and it is
/// what catches a CDN that answered with an HTML error page.
fn looks_like_jpeg(bytes: &[u8]) -> bool {
    bytes.len() > 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

/// One image at the configured size, decoded.
pub fn fetch_image(cfg: &FetchConfig, pano_id: &str) -> Result<image::RgbImage, String> {
    let http = Http::new(cfg)?;
    let layout = cfg.layout();
    let path = ensure_image(&http, cfg, &layout, pano_id, &BatchGuard::new(cfg))?;
    let bytes = cache::read_cached(&path).ok_or_else(|| format!("{pano_id}: cache entry gone"))?;
    match image::load_from_memory(&bytes) {
        Ok(img) => Ok(img.to_rgb8()),
        Err(e) => {
            // A file that will not decode can only poison every later run.
            let _ = std::fs::remove_file(&path);
            Err(format!("{pano_id}: {e}"))
        }
    }
}

// --------------------------------------------------------------------------- clusters

/// Downloads the OpenSfM clusters that are not cached yet.
pub fn download_clusters(cfg: &FetchConfig, clusters: &[ClusterRef]) -> Batch {
    let http = match Http::new(cfg) {
        Ok(h) => h,
        Err(e) => {
            return Batch {
                ready: BTreeMap::new(),
                failed: clusters.iter().map(|c| (c.id.clone(), e.clone())).collect(),
            }
        }
    };
    let layout = cfg.layout();
    let done = AtomicUsize::new(0);
    let total = clusters.len();
    emit_gui_progress_update(
        MESSAGE_ONLY,
        "Mapillary facades: downloading reconstructions...",
    );

    let guard = BatchGuard::new(cfg);
    let results: Vec<(String, Result<PathBuf, String>)> = in_pool(cfg.parallel, || {
        clusters
            .par_iter()
            .map(|cluster| {
                let outcome = ensure_cluster(&http, cfg, &layout, cluster, &guard);
                report(&done, total, "Downloading Mapillary reconstructions");
                (cluster.id.clone(), outcome)
            })
            .collect()
    });

    let mut batch = Batch::default();
    for (id, outcome) in results {
        match outcome {
            Ok(path) => {
                batch.ready.insert(id, path);
            }
            Err(e) => batch.failed.push((id, e)),
        }
    }
    batch
}

fn ensure_cluster(
    http: &Http,
    cfg: &FetchConfig,
    layout: &Layout,
    cluster: &ClusterRef,
    guard: &BatchGuard,
) -> Result<PathBuf, String> {
    let path = layout.cluster_path(&cluster.id)?;
    if cache::read_cached(&path).is_some() {
        // Cache first, guard second: see [`ensure_image`].
        guard.record(true);
        return Ok(path);
    }
    if let Some(why) = guard.stopped() {
        return Err(format!("cluster {}: {why}", cluster.id));
    }
    let outcome = download_cluster(http, cfg, layout, cluster, &path);
    guard.record(outcome.is_ok());
    outcome
}

/// One reconstruction off the network and into the cache.
fn download_cluster(
    http: &Http,
    cfg: &FetchConfig,
    layout: &Layout,
    cluster: &ClusterRef,
    path: &Path,
) -> Result<PathBuf, String> {
    let mut requeried = false;
    let mut record = match load_meta(layout, &cluster.pano_id) {
        Some(record) => record,
        None => {
            requeried = true;
            requery(http, cfg, layout, &cluster.pano_id)?
        }
    };
    let mut url = match cluster_url(&record) {
        Some(url) => url,
        None if !requeried => {
            requeried = true;
            record = requery(http, cfg, layout, &cluster.pano_id)?;
            cluster_url(&record).ok_or_else(|| format!("cluster {}: no URL", cluster.id))?
        }
        None => return Err(format!("cluster {}: no URL", cluster.id)),
    };

    loop {
        match http.get(&url, &[], false) {
            Ok(bytes) => {
                cache::write_atomic(path, &to_deflated(bytes)?)?;
                return Ok(path.to_path_buf());
            }
            Err(HttpError::Forbidden) if !requeried => {
                requeried = true;
                record = requery(http, cfg, layout, &cluster.pano_id)?;
                url = cluster_url(&record)
                    .ok_or_else(|| format!("cluster {}: no URL after re-query", cluster.id))?;
            }
            Err(e) => return Err(format!("cluster {}: {}", cluster.id, e.message())),
        }
    }
}

/// Normalises what the CDN sent to the zlib stream the cache stores.
///
/// The reconstruction arrives zlib compressed, but the Python tolerates a plain
/// body and so does this, and the cache is 2.7 times smaller compressed: the
/// Munich test box alone is 85 MB of clusters inflated against 31 MB deflated.
fn to_deflated(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if inflate(&bytes).is_ok() {
        return Ok(bytes);
    }
    if serde_json::from_slice::<Value>(&bytes).is_err() {
        return Err("cluster body is neither zlib nor JSON".to_string());
    }
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&bytes)
        .and_then(|()| encoder.finish())
        .map_err(|e| format!("deflate cluster: {e}"))
}

fn inflate(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .read_to_end(&mut out)
        .map_err(|e| format!("inflate: {e}"))?;
    Ok(out)
}

/// One cluster as JSON, downloading it if it is not cached.
pub fn fetch_cluster(cfg: &FetchConfig, cluster: &ClusterRef) -> Result<Value, String> {
    let http = Http::new(cfg)?;
    let layout = cfg.layout();
    let path = ensure_cluster(&http, cfg, &layout, cluster, &BatchGuard::new(cfg))?;
    let bytes =
        cache::read_cached(&path).ok_or_else(|| format!("cluster {}: cache gone", cluster.id))?;
    match parse_cluster_bytes(&bytes) {
        Ok(json) => Ok(json),
        Err(e) => {
            // A cluster that will not parse can only poison every later run.
            let _ = std::fs::remove_file(&path);
            Err(format!("cluster {}: {e}", cluster.id))
        }
    }
}

/// Inflates and parses a stored cluster.
pub fn parse_cluster_bytes(bytes: &[u8]) -> Result<Value, String> {
    let json = match inflate(bytes) {
        Ok(json) => json,
        // Tolerated for the same reason the Python tolerates it: a cluster that
        // was served uncompressed is still a usable cluster.
        Err(_) => bytes.to_vec(),
    };
    serde_json::from_slice(&json).map_err(|e| format!("cluster JSON: {e}"))
}

/// One shot of an OpenSfM reconstruction, exactly as the file carries it.
///
/// The values are still in the cluster's own topocentric frame; turning them
/// into run frame cameras is `sfm.rs`'s job.
#[derive(Clone, Debug, PartialEq)]
pub struct RawShot {
    pub shot_id: String,
    /// Axis-angle, world to camera, the same convention as `computed_rotation`.
    pub rotation: [f64; 3],
    pub translation: [f64; 3],
    /// Seconds; the Graph `captured_at` is this in milliseconds.
    pub capture_time: f64,
    /// The reconstruction's sequence key, which equals the Graph `sequence`.
    pub skey: String,
    pub compass: f64,
    /// The camera model key into the reconstruction's `cameras` table.
    pub camera: String,
}

/// One OpenSfM reconstruction, parsed but not yet moved into the run frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RawCluster {
    pub cluster_id: String,
    /// `(lon, lat, alt)` of the reconstruction's own origin, in the order
    /// `sfm::Cluster::ref_lla` wants.
    pub ref_lla: (f64, f64, f64),
    pub shots: Vec<RawShot>,
    pub points: Vec<[f64; 3]>,
    pub colors: Vec<[u8; 3]>,
}

impl RawCluster {
    /// Parses one reconstruction document.
    ///
    /// A file holds a list of reconstructions and the one with the most points
    /// wins, which is what the Python does. Points come back ordered by their
    /// numeric OpenSfM id: `serde_json` sorts object keys as strings and the
    /// file's own order is neither sorted nor recoverable, so the pipeline is
    /// given a stable order rather than the Python's.
    pub fn parse(doc: &Value, cluster_id: &str) -> Result<Self, String> {
        let recs: Vec<&Value> = match doc {
            Value::Array(a) => a.iter().collect(),
            Value::Object(_) => vec![doc],
            _ => return Err("reconstruction is neither a list nor an object".to_string()),
        };
        // The one with the most points wins, and the first of a tie, which is
        // what Python's `max` does over the same list.
        let mut best: Option<&Value> = None;
        let mut best_points = 0usize;
        for rec in recs {
            let points = rec
                .get("points")
                .and_then(Value::as_object)
                .map_or(0, serde_json::Map::len);
            if best.is_none() || points > best_points {
                best = Some(rec);
                best_points = points;
            }
        }
        let best = best.ok_or_else(|| "reconstruction file is empty".to_string())?;

        let lla = best
            .get("reference_lla")
            .ok_or_else(|| "reconstruction has no reference_lla".to_string())?;
        let num = |v: &Value, key: &str| v.get(key).and_then(Value::as_f64);
        let ref_lla = (
            num(lla, "longitude").ok_or("reference_lla has no longitude")?,
            num(lla, "latitude").ok_or("reference_lla has no latitude")?,
            num(lla, "altitude").unwrap_or(0.0),
        );

        let mut shots = Vec::new();
        if let Some(table) = best.get("shots").and_then(Value::as_object) {
            for (shot_id, shot) in table {
                let (Some(rotation), Some(translation)) =
                    (vec3(shot.get("rotation")), vec3(shot.get("translation")))
                else {
                    continue;
                };
                shots.push(RawShot {
                    shot_id: shot_id.clone(),
                    rotation,
                    translation,
                    capture_time: shot
                        .get("capture_time")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0),
                    skey: shot
                        .get("skey")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    compass: shot
                        .get("compass")
                        .and_then(|c| c.get("angle"))
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::NAN),
                    camera: shot
                        .get("camera")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }

        let mut keyed: Vec<(i64, [f64; 3], [u8; 3])> = Vec::new();
        if let Some(table) = best.get("points").and_then(Value::as_object) {
            for (id, point) in table {
                let Some(xyz) = vec3(point.get("coordinates")) else {
                    continue;
                };
                let color = vec3(point.get("color")).unwrap_or([0.0, 0.0, 0.0]);
                keyed.push((
                    id.parse::<i64>().unwrap_or(i64::MAX),
                    xyz,
                    [
                        color[0].round().clamp(0.0, 255.0) as u8,
                        color[1].round().clamp(0.0, 255.0) as u8,
                        color[2].round().clamp(0.0, 255.0) as u8,
                    ],
                ));
            }
        }
        keyed.sort_by_key(|(id, _, _)| *id);

        Ok(RawCluster {
            cluster_id: cluster_id.to_string(),
            ref_lla,
            shots,
            points: keyed.iter().map(|(_, p, _)| *p).collect(),
            colors: keyed.iter().map(|(_, _, c)| *c).collect(),
        })
    }

    /// The shot of one image, matched the way OpenSfM and the Graph API agree:
    /// `round(capture_time * 1000) == captured_at`. Exact and unique on the
    /// Munich cache.
    pub fn shot_for_captured_at(&self, captured_at: i64) -> Option<&RawShot> {
        self.shots
            .iter()
            .find(|s| (s.capture_time * 1000.0).round() as i64 == captured_at)
    }
}

fn vec3(v: Option<&Value>) -> Option<[f64; 3]> {
    let a = v?.as_array()?;
    Some([
        a.first()?.as_f64()?,
        a.get(1)?.as_f64()?,
        a.get(2)?.as_f64()?,
    ])
}

// --------------------------------------------------------------------------- OSM

/// Buildings in the bbox plus the OSM margin, from the first Overpass mirror
/// that answers.
///
/// The wider margin is not generosity: a building outside the world box still
/// occludes one inside it, and the line of sight test needs it as geometry.
pub fn fetch_osm(cfg: &FetchConfig) -> Result<Value, String> {
    let b = pad_bbox(cfg.bbox, cfg.osm_margin_m);
    let query = format!(
        "[out:json][timeout:60];\n(\n  way[\"building\"]({0},{1},{2},{3});\n  \
         relation[\"building\"]({0},{1},{2},{3});\n);\nout body; >; out skel qt;",
        b.min_lat, b.min_lon, b.max_lat, b.max_lon
    );

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(Duration::from_secs(120))
        .user_agent(crate::retrieve_data::OSM_USER_AGENT)
        .build()
        .map_err(|e| format!("Overpass HTTP client: {e}"))?;

    let mut last = String::new();
    for mirror in &cfg.endpoints.overpass {
        // GET with the query in `data`, the same shape `retrieve_data` sends,
        // so an Overpass mirror sees one client from Arnis and not two.
        let response = {
            let _permit = request_permit();
            client
                .get(mirror.as_str())
                .query(&[("data", query.as_str())])
                .send()
        };
        match response {
            Ok(r) if r.status().is_success() => match r.json::<Value>() {
                Ok(json) => return Ok(json),
                Err(e) => last = format!("{mirror}: {e}"),
            },
            Ok(r) => last = format!("{mirror}: HTTP {}", r.status()),
            Err(e) => last = format!("{mirror}: {e}"),
        }
        eprintln!("Note: Overpass mirror failed, trying the next ({last})");
    }
    Err(format!("every Overpass mirror failed; last: {last}"))
}

/// Imagery metadata and the OSM buildings, in that order.
///
/// Overpass runs last on purpose: an outage there must not cost the far slower
/// imagery search that already succeeded.
pub fn fetch(cfg: &FetchConfig) -> Result<Fetched, String> {
    let mut fetched = fetch_metadata(cfg)?;
    match fetch_osm(cfg) {
        Ok(osm) => fetched.osm = osm,
        Err(e) => eprintln!("Warning: OSM buildings not fetched: {e}"),
    }
    Ok(fetched)
}

// --------------------------------------------------------------------------- plumbing

/// Runs `body` on a pool of `threads`, so imagery downloads have their own
/// connection limit instead of taking the whole global pool.
fn in_pool<T: Send>(threads: usize, body: impl FnOnce() -> T + Send) -> T {
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads.clamp(1, 32))
        .build()
    {
        Ok(pool) => pool.install(body),
        // A pool that will not build is not a reason to skip the work.
        Err(_) => body(),
    }
}

/// Message-only progress, at most a hundred times per batch.
fn report(done: &AtomicUsize, total: usize, what: &str) {
    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
    let step = (total / 100).max(1);
    if n == total || n.is_multiple_of(step) {
        emit_gui_progress_update(
            MESSAGE_ONLY,
            &format!("Mapillary facades: {what} {n}/{total}..."),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    /// What the test server should do with one request.
    enum Act {
        Reply(u16, Vec<u8>),
        /// Announce a body far bigger than what is sent, then trickle a byte at
        /// a time and never finish: the throttled CDN this stage exists for.
        Trickle,
        /// Close the connection without answering, which is what a host that is
        /// not serving looks like from here.
        Hangup,
    }

    impl Act {
        fn text(status: u16, body: &str) -> Act {
            Act::Reply(status, body.as_bytes().to_vec())
        }
    }

    /// An HTTP server on loopback, one thread per connection so a stalled
    /// download cannot hold up the requests running beside it. Every request
    /// line it saw is kept, which is how the subdivision and re-query counts
    /// below are asserted. The handler is given the server's own base URL, so a
    /// reply can point at a path on this same server.
    struct Server {
        base: String,
        seen: Arc<Mutex<Vec<String>>>,
        listener: Arc<TcpListener>,
        stopping: Arc<AtomicBool>,
    }

    impl Server {
        fn new(handler: impl Fn(&str, &str, usize) -> Act + Send + Sync + 'static) -> Self {
            let listener = Arc::new(TcpListener::bind("127.0.0.1:0").unwrap());
            let base = format!("http://{}", listener.local_addr().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let stopping = Arc::new(AtomicBool::new(false));

            let handler = Arc::new(handler);
            let accept = Arc::clone(&listener);
            let log = Arc::clone(&seen);
            let stop = Arc::clone(&stopping);
            let own_base = base.clone();
            std::thread::spawn(move || {
                while let Ok((stream, _)) = accept.accept() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let (handler, log, base) =
                        (Arc::clone(&handler), Arc::clone(&log), own_base.clone());
                    std::thread::spawn(move || {
                        let Some(target) = read_request_line(&stream) else {
                            return;
                        };
                        let n = {
                            let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
                            log.push(target.clone());
                            log.len() - 1
                        };
                        serve(stream, handler(&target, &base, n));
                    });
                }
            });

            Server {
                base,
                seen,
                listener,
                stopping,
            }
        }

        fn hits(&self) -> Vec<String> {
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        fn count(&self, needle: &str) -> usize {
            self.hits().iter().filter(|h| h.contains(needle)).count()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            // Wake the accept loop so its thread ends with the test.
            self.stopping.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.listener.local_addr().unwrap());
        }
    }

    fn read_request_line(stream: &TcpStream) -> Option<String> {
        let mut buf = [0u8; 8192];
        let mut reader = stream;
        let read = reader.read(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf[..read]);
        text.lines().next().map(str::to_string)
    }

    fn serve(mut stream: TcpStream, act: Act) {
        match act {
            Act::Reply(status, body) => {
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
            Act::Trickle => {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4000000\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.flush();
                for _ in 0..200 {
                    if stream.write_all(b"\xff").is_err() || stream.flush().is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            Act::Hangup => drop(stream),
        }
    }

    /// A config pointed at a test server and a scratch cache, with the waits
    /// taken out so a retry costs a millisecond instead of five seconds.
    fn config(server: &Server, cache_dir: &std::path::Path) -> FetchConfig {
        let mut cfg = FetchConfig::new("MLY|test", BBox::new(48.1356, 11.5782, 48.1360, 11.5786));
        cfg.cache_dir = cache_dir.to_path_buf();
        cfg.pano_margin_m = 0.0;
        cfg.endpoints = Endpoints {
            images: format!("{}/images", server.base),
            image: server.base.clone(),
            overpass: vec![format!("{}/overpass", server.base)],
        };
        cfg.limits = Limits {
            attempts: 2,
            backoff: Duration::from_millis(1),
            subdivision_depth: 2,
        };
        cfg.parallel = 3;
        cfg
    }

    const TOO_LARGE: &str = "{\"error\":{\"message\":\"Please reduce the amount of data you are asking for, then retry your request\",\"code\":1}}";

    fn one_image(id: &str) -> String {
        serde_json::json!({
            "data": [{
                "id": id,
                "computed_geometry": {"type": "Point", "coordinates": [11.5804, 48.1367]},
                "computed_compass_angle": 322.47,
                "computed_rotation": [1.47, 0.41, -0.60],
                "computed_altitude": 4.24,
                "camera_type": "spherical",
                "is_pano": true,
                "quality_score": 0.94,
                "width": 5760,
                "height": 2880,
                "captured_at": 1_752_383_279_333i64,
                "sequence": "yM68owBapdtxI1DJP4s35l",
                "creator": {"username": "osmplus_org", "id": "1136057267544565"},
                "thumb_2048_url": "https://cdn.invalid/t.jpg",
                "sfm_cluster": {"id": "768264339100060", "url": "https://cdn.invalid/c"}
            }]
        })
        .to_string()
    }

    /// The permit counter in `net` is process wide and `net`'s own tests assert
    /// exact values of it, so every test here that really issues a request
    /// holds the shared lock first.
    fn serialized() -> std::sync::MutexGuard<'static, ()> {
        crate::net::PERMIT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// A one pixel JPEG, so a download can be checked without a real image.
    fn tiny_jpeg() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::from_pixel(1, 1, image::Rgb([9, 9, 9]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn the_tiler_subdivides_on_a_500_and_stops_at_the_depth_limit() {
        // Every cell is refused, so the tiler splits until the depth limit and
        // then counts the cell instead of splitting forever.
        let _serial = serialized();
        let server = Server::new(|_target, _base, _n| Act::text(500, TOO_LARGE));
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());

        let error = fetch_metadata(&cfg).unwrap_err();
        // 1 + 4 + 16: the root cell, its quadrants, and theirs. The sixteen at
        // the limit are counted rather than split again.
        assert_eq!(server.count("/images"), 21);
        // An area refused down to the smallest cell everywhere is not an
        // answer about coverage, so it does not come back as empty coverage.
        assert!(error.contains("16 search cells"), "{error}");
    }

    #[test]
    fn cells_refused_beside_cells_that_answer_are_a_hole_not_a_failure() {
        let _serial = serialized();
        // Two top-level cells, split on the meridian the bbox below straddles:
        // the western one answers, the eastern one is refused all the way down.
        let server = Server::new(|target, _base, _n| {
            if target.contains("bbox=11%2C") {
                Act::text(200, &one_image("1071917607838019"))
            } else {
                Act::text(500, TOO_LARGE)
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        // Wider than MAX_CELL_DEG, so `search_cells` gives two columns.
        cfg.bbox = BBox::new(48.10, 11.00, 48.105, 11.012);

        let fetched = fetch_metadata(&cfg).unwrap();
        assert_eq!(fetched.metas.len(), 1);
        // The refused column split twice, leaving sixteen leaves at the limit.
        assert_eq!(fetched.cells_refused, 16);
    }

    #[test]
    fn a_token_the_api_cannot_parse_is_not_retried_and_not_split() {
        let _serial = serialized();
        // What Mapillary answers a malformed token with: a 400, not a 401, and
        // an OAuthException in the body. Nothing about the status says the
        // request will never succeed, so the body has to.
        let server = Server::new(|_target, _base, _n| {
            Act::text(
                400,
                "{\"error\":{\"message\":\"Invalid OAuth access token - Cannot parse access token\",\
                 \"type\":\"OAuthException\",\"code\":190}}",
            )
        });
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());

        let error = fetch_metadata(&cfg).unwrap_err();
        assert!(error.contains("Mapillary token"), "{error}");
        let hits = server.count("/images");
        assert_eq!(hits, 1, "an auth failure was retried or split: {hits}");
    }

    #[test]
    fn a_refused_token_that_arrives_as_a_500_is_not_read_as_a_dense_cell() {
        let _serial = serialized();
        // A Graph 500 means "ask for less" whatever its body says, so a
        // refusal wearing one used to be answered by halving the cell, once
        // per sub-cell, and the area came out as a hole in coverage instead
        // of as the bad token it was.
        let server = Server::new(|_target, _base, _n| {
            Act::text(
                500,
                "{\"error\":{\"message\":\"Invalid OAuth access token\",\
                 \"type\":\"OAuthException\",\"code\":190}}",
            )
        });
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());

        let error = fetch_metadata(&cfg).unwrap_err();
        assert!(error.contains("Mapillary token"), "{error}");
        let hits = server.count("/images");
        assert_eq!(hits, 1, "the cell was split or retried: {hits} requests");
    }

    #[test]
    fn a_cell_that_answers_is_never_split() {
        let _serial = serialized();
        let server = Server::new(|target, _base, _n| {
            if target.contains("/images") {
                Act::text(200, &one_image("1071917607838019"))
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());

        let fetched = fetch_metadata(&cfg).unwrap();
        assert_eq!(server.count("/images"), 1);
        assert_eq!(fetched.cells_refused, 0);
        assert_eq!(fetched.metas.len(), 1);
        assert_eq!(fetched.metas[0].id, "1071917607838019");
        assert_eq!(fetched.metas[0].captured_at, 1_752_383_279_333);
        assert_eq!(fetched.clusters.len(), 1);
        assert_eq!(fetched.clusters[0].id, "768264339100060");
        assert_eq!(fetched.clusters[0].pano_id, "1071917607838019");
        assert_eq!(fetched.credits.len(), 1);
        assert_eq!(fetched.credits[0].creator, "osmplus_org");
        assert_eq!(
            fetched.credits[0].creator_url,
            "https://www.mapillary.com/app/user/osmplus_org"
        );
        assert_eq!(
            fetched.credits[0].image_url,
            "https://www.mapillary.com/app/?pKey=1071917607838019&focus=photo"
        );
        // The record is cached by id, so the next bbox can reuse it.
        assert!(load_meta(&cfg.layout(), "1071917607838019").is_some());
    }

    #[test]
    fn a_bad_token_fails_the_search_rather_than_reading_as_empty_coverage() {
        let _serial = serialized();
        let server = Server::new(|_target, _base, _n| {
            Act::text(
                401,
                "{\"error\":{\"message\":\"Invalid OAuth access token\"}}",
            )
        });
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());

        let error = fetch_metadata(&cfg).unwrap_err();
        assert!(error.contains("Invalid OAuth access token"), "{error}");
        // And it says what to do about it, because "Mapillary search: Mapillary
        // API 401" on its own does not.
        assert!(error.contains("Mapillary token"), "{error}");
        // A refused token is refused again, so the search must not spend the
        // retry budget on it: one request per cell, not `attempts` of them.
        let hits = server.count("/images");
        assert_eq!(hits, 1, "a 401 was retried: {hits} requests");
    }

    #[test]
    fn a_403_triggers_exactly_one_requery_and_the_fresh_url_is_used() {
        let jpeg = tiny_jpeg();
        let _serial = serialized();
        let server = Server::new(move |target, base, _n| {
            if target.contains("/expired.jpg") {
                Act::text(403, "")
            } else if target.contains("/fresh.jpg") {
                Act::Reply(200, jpeg.clone())
            } else if target.contains("/1071917607838019?") {
                Act::text(
                    200,
                    &serde_json::json!({
                        "id": "1071917607838019",
                        "thumb_2048_url": format!("{base}/fresh.jpg")
                    })
                    .to_string(),
                )
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());
        store_meta(
            &cfg.layout(),
            "1071917607838019",
            &serde_json::json!({
                "id": "1071917607838019",
                "thumb_2048_url": format!("{}/expired.jpg", server.base)
            }),
        );

        let batch = download_images(&cfg, &["1071917607838019".to_string()]);
        assert_eq!(batch.failed, Vec::new());
        assert_eq!(batch.ready.len(), 1);
        // The expired URL is tried once and never again, the image entity is
        // asked for fresh URLs exactly once, and the fresh URL delivers.
        assert_eq!(server.count("/expired.jpg"), 1, "{:?}", server.hits());
        assert_eq!(server.count("/1071917607838019?"), 1, "{:?}", server.hits());
        assert_eq!(server.count("/fresh.jpg"), 1);
        // The re-query merged into the cached record, so a later run starts
        // from the fresh URL rather than the expired one.
        let record = load_meta(&cfg.layout(), "1071917607838019").unwrap();
        assert!(record["thumb_2048_url"]
            .as_str()
            .unwrap()
            .ends_with("/fresh.jpg"));
    }

    #[test]
    fn a_second_403_is_not_re_queried_again() {
        // Both URLs are refused. One re-query is an expiry; a second would be a
        // loop, so the image is given up on instead.
        let _serial = serialized();
        let server = Server::new(|target, base, _n| {
            if target.contains(".jpg") {
                Act::text(403, "")
            } else if target.contains("/77?") {
                Act::text(
                    200,
                    &serde_json::json!({"id": "77", "thumb_2048_url": format!("{base}/second.jpg")})
                        .to_string(),
                )
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());
        store_meta(
            &cfg.layout(),
            "77",
            &serde_json::json!({"id": "77", "thumb_2048_url": format!("{}/first.jpg", server.base)}),
        );

        let batch = download_images(&cfg, &["77".to_string()]);
        assert_eq!(batch.ready.len(), 0);
        assert_eq!(batch.failed.len(), 1);
        assert_eq!(server.count("/77?"), 1, "one re-query only");
        assert_eq!(server.count("/first.jpg"), 1);
        assert_eq!(server.count("/second.jpg"), 1);
    }

    #[test]
    fn a_stalled_download_gives_up_without_killing_the_batch() {
        let jpeg = tiny_jpeg();
        let _serial = serialized();
        let server = Server::new(move |target, _base, _n| {
            if target.contains("/stall") {
                Act::Trickle
            } else if target.contains(".jpg") {
                Act::Reply(200, jpeg.clone())
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        cfg.deadline = Duration::from_millis(120);

        let ids = ["good_a", "stall_one", "good_b"];
        for id in ids {
            let url = if id.contains("stall") {
                format!("{}/stall", server.base)
            } else {
                format!("{}/{id}.jpg", server.base)
            };
            store_meta(
                &cfg.layout(),
                id,
                &serde_json::json!({"id": id, "thumb_2048_url": url}),
            );
        }

        let started = Instant::now();
        let batch = download_images(&cfg, &ids.map(str::to_string));

        assert_eq!(batch.ready.len(), 2, "the healthy downloads must survive");
        assert!(batch.ready.contains_key("good_a"));
        assert!(batch.ready.contains_key("good_b"));
        assert_eq!(batch.failed.len(), 1);
        assert_eq!(batch.failed[0].0, "stall_one");
        assert!(
            batch.failed[0].1.contains("stalled"),
            "{}",
            batch.failed[0].1
        );
        // Two attempts of 120 ms, not the four seconds the trickle would run.
        assert!(started.elapsed() < Duration::from_secs(3));
        // And the good ones are on disk under their id, ready for the next run.
        let path = cfg.layout().image_path("good_a", ImageSize::W2048).unwrap();
        assert!(cache::read_cached(&path).is_some());
    }

    /// A host that will not talk to us is not a server asking to be tried
    /// again. It used to buy the full five attempts and four growing backoffs,
    /// fifty seconds of sleeping per file, because a refused connection was
    /// counted differently from a stall.
    #[test]
    fn a_host_that_will_not_answer_buys_one_retry_not_four() {
        let _serial = serialized();
        let server = Server::new(move |target, _base, _n| {
            if target.contains("/dead.jpg") {
                Act::Hangup
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        // The shipped budget, with the sleeps taken out so the test is quick:
        // the point is how many attempts it makes, not how long it waits.
        cfg.limits.attempts = 5;
        store_meta(
            &cfg.layout(),
            "88",
            &serde_json::json!({"id": "88", "thumb_2048_url": format!("{}/dead.jpg", server.base)}),
        );

        let batch = download_images(&cfg, &["88".to_string()]);
        assert_eq!(batch.ready.len(), 0);
        assert_eq!(batch.failed.len(), 1);
        assert_eq!(
            server.count("/dead.jpg"),
            2,
            "one retry, then the host is written off: {:?}",
            server.hits()
        );
    }

    /// The imagery download sits in front of building generation, so a dead CDN
    /// used to hold a whole world hostage: five attempts and four backoffs each,
    /// six at a time, with nothing to stop the batch. One run was still retrying
    /// after 507 seconds and had to be killed.
    #[test]
    fn an_unreachable_host_stops_the_batch_instead_of_stalling_generation() {
        let _serial = serialized();
        let server = Server::new(|target, _base, _n| {
            if target.contains(".jpg") {
                Act::Hangup
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        cfg.limits.attempts = 5;
        cfg.parallel = DEFAULT_PARALLEL;

        let ids: Vec<String> = (0..60).map(|i| format!("{i:04}")).collect();
        for id in &ids {
            store_meta(
                &cfg.layout(),
                id,
                &serde_json::json!({
                    "id": id,
                    "thumb_2048_url": format!("{}/{id}.jpg", server.base),
                }),
            );
        }

        let batch = download_images(&cfg, &ids);

        assert_eq!(batch.ready.len(), 0, "the host delivers nothing");
        assert_eq!(batch.failed.len(), 60);
        let skipped = batch
            .failed
            .iter()
            .filter(|(_, why)| why.contains("in a row failed"))
            .count();
        assert!(
            skipped >= 60 - 2 * BATCH_GIVE_UP_FAILURES,
            "the batch must stop trying once it is clear nothing is coming: {skipped} of 60"
        );
        // Counted at the far end, which is the work this saves rather than a
        // wall clock that depends on how fast the machine is refused. Every
        // file the batch tries costs two requests and a backoff; trying all
        // sixty is 120 requests and ten waves of sleeping, which on the shipped
        // five second backoff is where the 507 second run came from.
        let tried = server.count(".jpg");
        assert!(
            tried <= 2 * (BATCH_GIVE_UP_FAILURES + DEFAULT_PARALLEL),
            "the batch kept knocking: {tried} requests for 60 files"
        );
    }

    /// Giving up on the network is not giving up on the cache.
    ///
    /// A second generation over an area already fetched asks for the same
    /// hundreds of images and finds them all on disk. If the batch had counted
    /// those against its patience, or checked its patience before looking, one
    /// run of bad luck would have cost the next run every image it already had.
    #[test]
    fn a_batch_that_has_given_up_still_hands_back_what_is_cached() {
        let _serial = serialized();
        let server = Server::new(|target, _base, _n| {
            if target.contains(".jpg") {
                Act::Hangup
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        cfg.parallel = 1;

        // Enough dead files to use up the batch's patience twice over, and then
        // one that is already on disk.
        let mut ids: Vec<String> = (0..BATCH_GIVE_UP_FAILURES * 2)
            .map(|i| format!("dead{i}"))
            .collect();
        for id in &ids {
            store_meta(
                &cfg.layout(),
                id,
                &serde_json::json!({
                    "id": id,
                    "thumb_2048_url": format!("{}/{id}.jpg", server.base),
                }),
            );
        }
        let cached = cfg.layout().image_path("9999", ImageSize::W2048).unwrap();
        cache::write_atomic(&cached, &tiny_jpeg()).unwrap();
        ids.push("9999".to_string());

        let batch = download_images(&cfg, &ids);
        assert_eq!(
            batch.path("9999"),
            Some(&cached),
            "the cached image must come back however badly the batch went"
        );
        assert_eq!(batch.failed.len(), BATCH_GIVE_UP_FAILURES * 2);
    }

    /// What an unreachable CDN actually costs a generation, on the shipped
    /// [`Limits`], measured rather than reasoned about. Ignored because it is a
    /// stopwatch and not an assertion:
    ///
    /// ```text
    /// cargo test --target-dir target-plane -- --ignored --nocapture dead_cdn
    /// ```
    #[test]
    #[ignore = "a wall clock measurement against a dead port"]
    fn what_a_dead_cdn_costs_a_generation() {
        let _serial = serialized();
        // A port nothing is listening on: bound to learn a free one, then
        // dropped, so connections there are refused.
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let tmp = tempfile::tempdir().unwrap();
        let server = Server::new(|_t, _b, _n| Act::text(404, ""));
        let mut cfg = config(&server, tmp.path());
        cfg.limits = Limits::default();
        cfg.parallel = DEFAULT_PARALLEL;

        let ids: Vec<String> = (0..60).map(|i| format!("{i:04}")).collect();
        for id in &ids {
            store_meta(
                &cfg.layout(),
                id,
                &serde_json::json!({"id": id, "thumb_2048_url": format!("http://{dead}/{id}.jpg")}),
            );
        }

        let started = Instant::now();
        let batch = download_images(&cfg, &ids);
        let elapsed = started.elapsed();
        let given_up = batch
            .failed
            .iter()
            .filter(|(_, why)| why.contains("in a row failed"))
            .count();
        let tried = 60 - given_up;
        // What it used to be: every file got `attempts` tries with a backoff of
        // 5, 10, 15 and 20 seconds between them, six files at a time.
        let sleeps: f64 = (0..cfg.limits.attempts - 1)
            .map(|k| cfg.limits.backoff.as_secs_f64() * f64::from(k + 1))
            .sum();
        println!(
            "60 unreachable files: {:.1} s, {tried} tried and {} given up on; \
             the old budget was {:.0} s of sleeping per file, {:.0} s for the batch",
            elapsed.as_secs_f64(),
            given_up,
            sleeps,
            sleeps * (60.0 / DEFAULT_PARALLEL as f64),
        );
        assert_eq!(batch.ready.len(), 0);
    }

    #[test]
    fn a_cached_image_is_not_downloaded_again() {
        let jpeg = tiny_jpeg();
        let _serial = serialized();
        let server = Server::new(move |_target, _base, _n| Act::Reply(200, jpeg.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());
        store_meta(
            &cfg.layout(),
            "42",
            &serde_json::json!({"id": "42", "thumb_2048_url": format!("{}/a.jpg", server.base)}),
        );

        assert_eq!(download_images(&cfg, &["42".to_string()]).ready.len(), 1);
        assert_eq!(server.count("/a.jpg"), 1);
        assert_eq!(download_images(&cfg, &["42".to_string()]).ready.len(), 1);
        assert_eq!(server.count("/a.jpg"), 1, "the second run reads the cache");
    }

    /// The texture stage's `hires` ladder is only worth anything if asking for
    /// `ImageSize::Original` actually fetches the full resolution URL and files
    /// it beside the thumbnail rather than over it. Both sizes of one pano live
    /// in the cache at once, because a wall reads the original and the image
    /// gates read the thumbnail.
    #[test]
    fn an_original_is_fetched_from_its_own_url_and_cached_beside_the_thumbnail() {
        let jpeg = tiny_jpeg();
        let _serial = serialized();
        let server = Server::new(move |_target, _base, _n| Act::Reply(200, jpeg.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        store_meta(
            &cfg.layout(),
            "42",
            &serde_json::json!({
                "id": "42",
                "thumb_2048_url": format!("{}/small.jpg", server.base),
                "thumb_original_url": format!("{}/big.jpg", server.base),
            }),
        );

        assert_eq!(download_images(&cfg, &["42".to_string()]).ready.len(), 1);
        cfg.size = ImageSize::Original;
        assert_eq!(download_images(&cfg, &["42".to_string()]).ready.len(), 1);
        assert_eq!(server.count("/small.jpg"), 1, "the thumbnail's own URL");
        assert_eq!(server.count("/big.jpg"), 1, "the original's own URL");
        let layout = cfg.layout();
        for size in [ImageSize::W2048, ImageSize::Original] {
            let path = layout.image_path("42", size).unwrap();
            assert!(
                cache::read_cached(&path).is_some(),
                "{} is not cached",
                path.display()
            );
        }
    }

    #[test]
    fn a_body_that_is_not_a_jpeg_is_refused_rather_than_cached() {
        let _serial = serialized();
        let server = Server::new(|_target, _base, _n| Act::text(200, "<html>rate limited</html>"));
        let tmp = tempfile::tempdir().unwrap();
        let cfg = config(&server, tmp.path());
        store_meta(
            &cfg.layout(),
            "42",
            &serde_json::json!({"id": "42", "thumb_2048_url": format!("{}/a.jpg", server.base)}),
        );

        let batch = download_images(&cfg, &["42".to_string()]);
        assert_eq!(batch.failed.len(), 1);
        assert!(
            batch.failed[0].1.contains("not a JPEG"),
            "{:?}",
            batch.failed
        );
        let path = cfg.layout().image_path("42", ImageSize::W2048).unwrap();
        assert!(cache::read_cached(&path).is_none());
    }

    #[test]
    fn a_cluster_round_trips_through_the_cache_compressed() {
        let doc = serde_json::json!([{
            "reference_lla": {"latitude": 48.137873, "longitude": 11.586092, "altitude": 0.0},
            "shots": {
                "zv1sjal7pfRw26gBWSLN8H": {
                    "rotation": [1.4002624, 0.9208540, -0.8735068],
                    "translation": [-13.5354629, 0.7187403, 36.5540530],
                    "camera": "v2 google pixel 6 4080 3072 perspective 0.0",
                    "capture_time": 1_716_405_108.93,
                    "compass": {"angle": 225.206, "accuracy": -1.0},
                    "skey": "m5N0ZeTPxABg9tlIO1sa3G"
                }
            },
            "points": {
                "151": {"color": [45.0, 57.0, 45.0], "coordinates": [179.877, -25.447, 6.158]},
                "12": {"color": [255.0, 0.0, 128.4], "coordinates": [1.0, 2.0, 3.0]}
            }
        }]);
        let plain = serde_json::to_vec(&doc).unwrap();
        let deflated = to_deflated(plain.clone()).unwrap();
        assert!(inflate(&deflated).is_ok(), "the cache holds a zlib stream");
        assert!(deflated.len() < plain.len());
        // Already compressed bytes are stored as they arrived.
        assert_eq!(to_deflated(deflated.clone()).unwrap(), deflated);

        let parsed = parse_cluster_bytes(&deflated).unwrap();
        let cluster = RawCluster::parse(&parsed, "768264339100060").unwrap();
        assert_eq!(cluster.cluster_id, "768264339100060");
        assert_eq!(cluster.ref_lla.0, 11.586092);
        assert_eq!(cluster.ref_lla.1, 48.137873);
        assert_eq!(cluster.shots.len(), 1);
        assert_eq!(cluster.shots[0].skey, "m5N0ZeTPxABg9tlIO1sa3G");
        assert_eq!(cluster.shots[0].translation[2], 36.5540530);
        assert_eq!(cluster.shots[0].rotation[0], 1.4002624);
        // Points come back in numeric id order and their colours as bytes.
        assert_eq!(cluster.points.len(), 2);
        assert_eq!(cluster.points[0], [1.0, 2.0, 3.0]);
        assert_eq!(cluster.colors[0], [255, 0, 128]);
        assert_eq!(cluster.colors[1], [45, 57, 45]);
    }

    #[test]
    fn a_shot_is_matched_to_its_image_by_the_capture_millisecond() {
        let doc = serde_json::json!({
            "reference_lla": {"latitude": 48.0, "longitude": 11.0, "altitude": 0.0},
            "shots": {
                "a": {"rotation": [0.0, 0.0, 0.0], "translation": [0.0, 0.0, 0.0],
                      "capture_time": 1_716_405_108.93},
                "b": {"rotation": [0.0, 0.0, 0.0], "translation": [1.0, 0.0, 0.0],
                      "capture_time": 1_716_405_109.93}
            },
            "points": {}
        });
        let cluster = RawCluster::parse(&doc, "c").unwrap();
        assert_eq!(
            cluster
                .shot_for_captured_at(1_716_405_108_930)
                .unwrap()
                .shot_id,
            "a"
        );
        assert_eq!(
            cluster
                .shot_for_captured_at(1_716_405_109_930)
                .unwrap()
                .shot_id,
            "b"
        );
        assert!(cluster.shot_for_captured_at(1_716_405_108_931).is_none());
    }

    #[test]
    fn the_reconstruction_with_the_most_points_wins() {
        let doc = serde_json::json!([
            {"reference_lla": {"latitude": 1.0, "longitude": 2.0, "altitude": 0.0},
             "shots": {}, "points": {"1": {"coordinates": [0.0, 0.0, 0.0]}}},
            {"reference_lla": {"latitude": 3.0, "longitude": 4.0, "altitude": 0.0},
             "shots": {}, "points": {"1": {"coordinates": [0.0, 0.0, 0.0]},
                                     "2": {"coordinates": [1.0, 1.0, 1.0]}}}
        ]);
        let cluster = RawCluster::parse(&doc, "c").unwrap();
        assert_eq!(cluster.ref_lla, (4.0, 3.0, 0.0));
        assert_eq!(cluster.points.len(), 2);
    }

    #[test]
    fn the_margins_grow_the_box_by_metres_on_every_side() {
        let bbox = BBox::new(48.135635, 11.578243, 48.137225, 11.580818);
        let padded = pad_bbox(bbox, 45.0);
        // 45 m of latitude is 0.000405 degrees anywhere on earth, and at 48
        // degrees north a metre of longitude is 1.5 times as many degrees.
        assert!((bbox.min_lat - padded.min_lat - 0.000_404_7).abs() < 1e-6);
        assert!((padded.max_lat - bbox.max_lat - 0.000_404_7).abs() < 1e-6);
        assert!((bbox.min_lon - padded.min_lon - 0.000_605).abs() < 1e-5);
        // The OSM margin is the wider one: an outside building still occludes.
        let osm = pad_bbox(bbox, 60.0);
        assert!(osm.min_lat < padded.min_lat && osm.max_lon > padded.max_lon);
    }

    #[test]
    fn a_credit_falls_back_to_naming_the_image_when_there_is_no_place_name() {
        let mut cfg = FetchConfig::new("t", BBox::new(0.0, 0.0, 1.0, 1.0));
        let record = serde_json::json!({"id": "7", "creator": {"username": "", "id": "42"}});

        let anonymous = credit_from(&cfg, &record, "7");
        assert_eq!(anonymous.title, "Mapillary image 7");
        // No username, so the profile link falls back to the numeric id, which
        // Mapillary routes as well.
        assert_eq!(
            anonymous.creator_url,
            "https://www.mapillary.com/app/user/42"
        );

        cfg.area_label = Some("Munich, Germany".to_string());
        assert_eq!(credit_from(&cfg, &record, "7").title, "Munich, Germany");
    }

    #[test]
    fn a_metadata_record_is_merged_and_never_loses_its_pose_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::new(tmp.path().to_path_buf());
        store_meta(
            &layout,
            "7",
            &serde_json::json!({"id": "7", "computed_rotation": [1.0, 2.0, 3.0],
                                "thumb_2048_url": "http://old"}),
        );
        // A re-query only asks for the URL fields; the pose must survive it.
        store_meta(
            &layout,
            "7",
            &serde_json::json!({"id": "7", "thumb_2048_url": "http://new"}),
        );
        let record = load_meta(&layout, "7").unwrap();
        assert_eq!(record["computed_rotation"][2], 3.0);
        assert_eq!(record["thumb_2048_url"], "http://new");
    }

    #[test]
    fn camera_classes_outside_the_run_are_cached_but_not_returned() {
        let _serial = serialized();
        let server = Server::new(|target, _base, _n| {
            if target.contains("/images") {
                Act::text(
                    200,
                    &serde_json::json!({"data": [
                        {"id": "1", "camera_type": "spherical",
                         "computed_geometry": {"coordinates": [11.58, 48.136]}},
                        {"id": "2", "camera_type": "perspective",
                         "computed_geometry": {"coordinates": [11.58, 48.136]}}
                    ]})
                    .to_string(),
                )
            } else {
                Act::text(404, "")
            }
        });
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config(&server, tmp.path());
        cfg.camera_types = vec![CameraModel::Spherical];

        let fetched = fetch_metadata(&cfg).unwrap();
        assert_eq!(fetched.metas.len(), 1);
        assert_eq!(fetched.metas[0].id, "1");
        // Both are on disk: metadata is cheap and a later run may want phones.
        assert!(load_meta(&cfg.layout(), "2").is_some());
    }

    #[test]
    fn the_default_depth_limit_is_the_python_one() {
        // The reference run was produced with five levels of subdivision, so a
        // dense centre must not be given up on any earlier here than there.
        assert_eq!(Limits::default().subdivision_depth, 5);
        assert_eq!(Limits::default().attempts, 5);
        assert_eq!(
            FetchConfig::new("t", BBox::new(0.0, 0.0, 1.0, 1.0)).size,
            ImageSize::W2048
        );
    }

    /// The real Graph API over the Munich fixture box. Ignored by default; run
    /// it with a token when the field list or the search has been touched:
    ///
    /// ```text
    /// set MAPILLARY_TOKEN=MLY|...
    /// cargo test --target-dir target-test live_munich -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_munich_search_finds_the_fixture_images() {
        let _serial = serialized();
        let Ok(token) = std::env::var("MAPILLARY_TOKEN") else {
            panic!("set MAPILLARY_TOKEN to run this");
        };
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg =
            FetchConfig::new(token, BBox::new(48.135635, 11.578243, 48.137225, 11.580818));
        cfg.cache_dir = tmp.path().to_path_buf();
        cfg.area_label = Some("Munich, Germany".to_string());

        let fetched = fetch_metadata(&cfg).unwrap();
        let spherical = fetched.metas.iter().filter(|m| m.is_spherical()).count();
        println!(
            "{} images ({spherical} spherical), {} clusters, {} cells refused",
            fetched.metas.len(),
            fetched.clusters.len(),
            fetched.cells_refused
        );
        println!("first credit: {:?}", fetched.credits.first());

        // The fixture box holds 1076 images of which 288 are spherical; the
        // count drifts as people upload, so this only asks for the right order.
        assert!(fetched.metas.len() > 900, "{}", fetched.metas.len());
        assert!(spherical > 200, "{spherical}");
        assert!(fetched.clusters.len() > 50);
        assert!(fetched.credits.iter().all(|c| !c.creator.is_empty()));

        // Every field the pipeline reads has to have survived the round trip.
        let with_rotation = fetched
            .metas
            .iter()
            .filter(|m| m.rotation.is_some())
            .count();
        assert!(with_rotation > 900, "{with_rotation}");
        assert!(fetched.metas.iter().all(|m| m.captured_at > 0));
        assert!(fetched.metas.iter().all(|m| m.width > 0 && m.height > 0));

        // One image and one cluster all the way through, so the download path
        // and the zlib inflate are exercised against the real CDN.
        let meta = fetched
            .metas
            .iter()
            .find(|m| m.is_spherical())
            .expect("a panorama");
        let image = fetch_image(&cfg, &meta.id).unwrap();
        assert_eq!(image.width(), 2048, "the 2048 thumb is 2048 wide");
        assert_eq!(image.height(), 1024, "and a panorama is 2:1");

        let cluster_ref = &fetched.clusters[0];
        let doc = fetch_cluster(&cfg, cluster_ref).unwrap();
        let cluster = RawCluster::parse(&doc, &cluster_ref.id).unwrap();
        println!(
            "cluster {}: {} points, {} shots",
            cluster.cluster_id,
            cluster.points.len(),
            cluster.shots.len()
        );
        assert!(!cluster.points.is_empty());
        assert!(!cluster.shots.is_empty());
        // The shot of the image that named this cluster must be findable by
        // the capture millisecond, which is the whole shot to image mapping.
        let owner = fetched
            .metas
            .iter()
            .find(|m| m.id == cluster_ref.pano_id)
            .unwrap();
        assert!(
            cluster.shot_for_captured_at(owner.captured_at).is_some(),
            "no shot at captured_at {}",
            owner.captured_at
        );

        println!(
            "cache holds {}",
            cache::format_size(cfg.layout().size_bytes())
        );
    }
}
