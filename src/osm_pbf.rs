//! Reads OSM data from an `.osm.pbf` extract instead of the tile archive (`--osm-pbf`).
//!
//! The extract is a local file, or the smallest Geofabrik region whose border holds the
//! selection, downloaded once. A run cuts its area out of it the way the Overpass query
//! would and keeps the cut on disk (the bake), so a repeat run, and every piece of a job,
//! reads the bake instead of the extract.

use crate::args::Args;
use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::{OsmData, OsmElement, OsmMember};
use crate::progress::emit_gui_progress_update;
use crate::transfer::{RateMeter, Stage, Transfer};
use colored::Colorize;
use osmpbf::{BlobDecode, BlobReader, Element, PrimitiveBlock, RelMemberType};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `--osm-pbf geofabrik`: the extract is picked from the selection and downloaded.
pub const GEOFABRIK: &str = "geofabrik";
const GEOFABRIK_INDEX_URL: &str = "https://download.geofabrik.de/index-v1.json";
/// Extract borders move over years; a week-old index names the same files.
const INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 86_400);

type Result<T> = std::result::Result<T, String>;

/// What `--osm-pbf` and `--osm-pbf-url` ask for.
pub struct Source {
    spec: String,
    url: Option<String>,
    /// Kept past the bbox: the One World clip pad, in metres at this scale.
    pad_m: f64,
}

impl Source {
    pub fn from_args(args: &Args) -> Option<Self> {
        args.osm_pbf.as_ref().map(|spec| Source {
            spec: spec.clone(),
            url: args.osm_pbf_url.clone(),
            pad_m: f64::from(crate::one_world::CLIP_PAD_BLOCKS) / args.scale.max(1e-6),
        })
    }
}

/// A failed Geofabrik download may fall back to the default source; a file the user
/// named, or `--offline`, keeps the error.
pub fn fallback_allowed(src: &Source, offline: bool) -> bool {
    src.spec == GEOFABRIK && !offline
}

/// The OSM data of `bbox`, cut from a bake that covers it, or from the extract.
pub fn fetch_data_from_pbf(src: &Source, bbox: LLBBox) -> Result<OsmData> {
    println!("{} Reading data from the .pbf extract...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Reading the .pbf extract...");
    let want = E7Box::around(&bbox, src.pad_m);
    let (bake, of) = bake(src, want)?;
    emit_gui_progress_update(5.0, "");
    // A bake of exactly `want` is already its cut.
    let data = if of == want {
        bake
    } else {
        cut(bake.elements(), want)
    };
    if data.is_empty() {
        no_data(src)?;
    }
    Ok(data)
}

/// Bakes a whole job before its pieces start, so each piece cuts its own area from the
/// bake instead of reading the extract again.
pub fn bake_for_job(src: &Source, selection: LLBBox) -> Result<()> {
    // Twice the pad: a piece first snaps outward to the chunk grid (under 16 blocks), then
    // adds its own pad.
    let (data, _) = bake(src, E7Box::around(&selection, 2.0 * src.pad_m))?;
    if data.is_empty() {
        no_data(src)?;
    }
    Ok(())
}

/// Nothing in the area most likely means the extract does not cover it: Region Download
/// falls back to the default source, a file the user named only warns.
fn no_data(src: &Source) -> Result<()> {
    let e = "the extract holds no OpenStreetMap data for this area";
    if fallback_allowed(src, crate::net::offline()) {
        return Err(e.to_string());
    }
    eprintln!(
        "{}",
        format!("Warning: {e}; check that it covers the selection.")
            .yellow()
            .bold()
    );
    Ok(())
}

fn cache_root() -> Result<PathBuf> {
    crate::elevation::cache::user_cache_dir()
        .map(|d| cache_root_in(&d))
        .ok_or_else(|| "no cache directory for the .pbf bake".to_string())
}

/// The extract cache under the cache root `root`.
fn cache_root_in(root: &Path) -> PathBuf {
    root.join("arnis").join("osm-pbf")
}

/// The extract cache, for the settings panel's size and Clear Cache.
pub fn cache_dir() -> Option<PathBuf> {
    crate::elevation::cache::user_cache_dir().map(|d| cache_root_in(&d))
}

/// Clears the downloaded extracts and their bakes.
pub fn clear_osm_pbf_cache() -> crate::elevation::cache::CacheClearStats {
    match cache_dir() {
        Some(d) => crate::elevation::cache::clear_cache_dir(&d),
        None => Default::default(),
    }
}

/// What `--osm-pbf` needs for a selection, as the caches hold it.
#[derive(serde::Serialize, Debug, Default, PartialEq)]
pub struct ExtractPlan {
    /// The Geofabrik region, or the file's name; `None` when no cached index
    /// names one.
    pub name: Option<String>,
    /// Size on disk, once downloaded.
    pub bytes: Option<u64>,
    pub downloaded: bool,
    /// A bake holding the selection is on disk.
    pub baked: bool,
    /// Where a Geofabrik extract downloads from.
    pub url: Option<String>,
    /// The download size: on disk, else as a download or HEAD last saw it.
    pub download_bytes: Option<u64>,
    /// The bake holding the selection, on disk.
    pub bake_bytes: Option<u64>,
    /// What a bake of the selection would take on disk, and in memory while
    /// it runs; estimates from the download size.
    pub bake_estimate: Option<u64>,
    pub ram_estimate: Option<u64>,
}

/// [`ExtractPlan`] for `selection` from the caches under `root` alone: the
/// Geofabrik index is read only from disk, and nothing is downloaded.
pub fn plan(root: &Path, src: &Source, selection: LLBBox) -> ExtractPlan {
    let cache = cache_root_in(root);
    let want = E7Box::around(&selection, src.pad_m);
    let mut url_of = None;
    let mut share = None;
    let (name, pbf) = if src.spec == GEOFABRIK {
        let index = std::fs::read(cache.join("geofabrik-index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        let picked = match (&src.url, &index) {
            (Some(url), _) => Some((None, url.clone())),
            (None, Some(index)) => geofabrik_feature(index, &want).ok().map(|f| {
                share = Some(want.area_deg2() / rings_area(&rings(&f["geometry"])).max(1e-12));
                let name = f["properties"]["name"].as_str().map(str::to_string);
                let url = f["properties"]["urls"]["pbf"].as_str().unwrap_or_default();
                (name, url.to_string())
            }),
            (None, None) => None,
        };
        let Some((region, url)) = picked else {
            return ExtractPlan::default();
        };
        let Some(dest) = download_path(&cache, &url) else {
            return ExtractPlan::default();
        };
        let region = region.or_else(|| download_name(&url).map(str::to_string));
        url_of = Some(url.clone());
        (region, dest)
    } else {
        let p = PathBuf::from(&src.spec);
        (p.file_name().map(|n| n.to_string_lossy().into_owned()), p)
    };
    let bytes = std::fs::metadata(&pbf)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    let bake = bytes
        .and_then(|_| pbf_key(&pbf).ok())
        .and_then(|key| find_bake(&cache.join("bakes").join(key), &want))
        .map(|(path, _)| path);
    let bake_bytes = bake
        .and_then(|b| std::fs::metadata(b).ok())
        .map(|m| m.len());
    let download_bytes = bytes.or_else(|| {
        let name = pbf.file_name()?.to_string_lossy().into_owned();
        read_sizes(pbf.parent()?)
            .get(&name)
            .copied()
            .filter(|&b| b > 0)
    });
    ExtractPlan {
        name,
        bytes,
        downloaded: bytes.is_some(),
        baked: bake_bytes.is_some(),
        url: url_of,
        download_bytes,
        bake_bytes,
        bake_estimate: download_bytes
            .zip(share)
            .map(|(pbf, share)| crate::data_plan::region_bake_bytes(pbf, share)),
        ram_estimate: download_bytes.map(crate::data_plan::bake_ram_bytes),
    }
}

/// The bake of `want`, read from disk when an earlier one covers it, and the box it holds.
fn bake(src: &Source, want: E7Box) -> Result<(OsmData, E7Box)> {
    let pbf = locate(src, &want)?;
    let dir = cache_root()?.join("bakes").join(pbf_key(&pbf)?);
    if let Some((hit, of)) = find_bake(&dir, &want) {
        println!("Reading the bake {}", hit.display());
        return Ok((read_bake(&hit)?, of));
    }
    let t = Instant::now();
    let data = cut_pbf(&pbf, want)?;
    let label = extract_label(&pbf);
    report_bake(&label, Stage::Finalize, 0.0, 0, 0);
    println!(
        "Baked {} elements from {} in {:.1}s on {} threads",
        data.elements().len(),
        pbf.display(),
        t.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );
    // An empty cut is not kept: the extract most likely misses the area.
    if data.is_empty() {
        report_bake(&label, Stage::Finalize, 1.0, 0, 0);
        return Ok((data, want));
    }
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 3).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut enc, &data).map_err(|e| e.to_string())?;
    let bytes = enc.finish().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    crate::world_utils::replace_file_atomically(&dir.join(want.file_name()), &bytes)?;
    report_bake(
        &label,
        Stage::Finalize,
        1.0,
        bytes.len() as u64,
        bytes.len() as u64,
    );
    Ok((data, want))
}

/// The extract's short name: `liechtenstein` for `liechtenstein-latest.osm.pbf`.
fn extract_label(pbf: &Path) -> String {
    let name = pbf
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.strip_suffix(".pbf").unwrap_or(&name);
    let name = name.strip_suffix(".osm").unwrap_or(name);
    name.strip_suffix("-latest").unwrap_or(name).to_string()
}

/// Passes over the extract a bake may take; the third runs only when a
/// relation needs ways from outside the area, so the bar may skip it.
const BAKE_PASSES: f64 = 3.0;

/// A Region Download bake is `frac` through `stage`, `done` of `total` bytes.
fn report_bake(label: &str, stage: Stage, frac: f64, done: u64, total: u64) {
    let threads = rayon::current_num_threads();
    crate::progress::emit_gui_transfer(
        crate::progress::MESSAGE_ONLY,
        "",
        &Transfer {
            stage,
            name: label.to_string(),
            item: 1,
            items: 1,
            done_bytes: done,
            total_bytes: total,
            threads,
            cpu_pct: crate::transfer::cpu_pct(threads, crate::transfer::cores()),
            downloads: crate::net::max_requests() as u32,
            percent: crate::transfer::job_percent(0, 1, stage, frac),
            ..Transfer::default()
        },
    );
}

fn read_bake(path: &Path) -> Result<OsmData> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dec = zstd::stream::Decoder::new(file).map_err(|e| e.to_string())?;
    serde_json::from_reader(std::io::BufReader::new(dec))
        .map_err(|e| format!("bad bake {}: {e}", path.display()))
}

/// The smallest bake in `dir` holding all of `want`, and its box.
fn find_bake(dir: &Path, want: &E7Box) -> Option<(PathBuf, E7Box)> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let b = E7Box::from_file_name(&name)?;
            b.contains(want).then(|| (e.path(), b))
        })
        .min_by_key(|(_, b)| b.area())
}

/// Names one version of an extract: a re-download gets a new size or time, and its bakes.
fn pbf_key(pbf: &Path) -> Result<String> {
    let md = std::fs::metadata(pbf).map_err(|e| format!("{}: {e}", pbf.display()))?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let name = pbf.file_name().map(|n| n.to_string_lossy().into_owned());
    let digest = Sha256::digest(format!("{name:?}|{}|{mtime}", md.len()));
    Ok(digest[..8].iter().map(|b| format!("{b:02x}")).collect())
}

// ── the extract ───────────────────────────────────────────────────────────────

/// The extract on disk, downloaded first for `geofabrik`.
fn locate(src: &Source, want: &E7Box) -> Result<PathBuf> {
    if src.spec != GEOFABRIK {
        let p = PathBuf::from(&src.spec);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("--osm-pbf: {} is not a file", p.display()))
        };
    }
    // A picked extract is held to a size the bake can hold in memory; a named one is not.
    let (url, cap) = match &src.url {
        Some(url) => (url.clone(), None),
        None => (
            geofabrik_url(&fetch_index()?, want)?,
            Some(MAX_PICKED_EXTRACT_BYTES),
        ),
    };
    let dest =
        download_path(&cache_root()?, &url).ok_or_else(|| format!("not a .pbf url: {url}"))?;
    if !downloaded_whole(&dest) {
        download(&url, &dest, cap)?;
    } else if let Some(cap) = cap {
        let bytes = std::fs::metadata(&dest).map_or(0, |m| m.len());
        check_extract_size(&extract_label(&dest), bytes, cap)?;
    }
    Ok(dest)
}

/// Largest extract `--osm-pbf geofabrik` picks on its own: a bake peaks near 4.5x the file in
/// memory, so Germany (4.4 GB) or Europe (31 GB) would run out.
const MAX_PICKED_EXTRACT_BYTES: u64 = 1_500_000_000;

fn check_extract_size(label: &str, bytes: u64, cap: u64) -> Result<()> {
    if bytes <= cap {
        return Ok(());
    }
    Err(format!(
        "the smallest Geofabrik extract holding the selection, {label}, is {:.1} GB, past the {:.1} GB \
         Arnis picks on its own; name a smaller one with --osm-pbf-url or pass a .pbf file",
        bytes as f64 / 1e9,
        cap as f64 / 1e9
    ))
}

/// Where the extract at `url` is kept: in a folder named for the whole url, so two regions
/// sharing a file name (the US state of Georgia and the country) never share a download.
pub(crate) fn download_path(cache: &Path, url: &str) -> Option<PathBuf> {
    let name = download_name(url)?;
    let key: String = Sha256::digest(url.as_bytes())[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(cache.join("downloads").join(key).join(name))
}

/// Whether the extract is at `dest` and as long as its server declared, when it declared it:
/// a short one is fetched again.
fn downloaded_whole(dest: &Path) -> bool {
    let Ok(m) = std::fs::metadata(dest) else {
        return false;
    };
    let declared = dest
        .parent()
        .zip(dest.file_name())
        .and_then(|(dir, name)| read_sizes(dir).get(&*name.to_string_lossy()).copied());
    m.is_file() && declared.is_none_or(|bytes| m.len() == bytes)
}

/// A download's partial file, one per process: two runs fetching one extract never share it.
fn part_path(dest: &Path) -> PathBuf {
    dest.with_extension(format!("pbf.{}.part", std::process::id()))
}

/// Removes the partial files a killed download left beside `dest`. A running one writes at
/// least once per read timeout, so a day without a write means none is.
fn remove_stale_parts(dest: &Path) {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name()) else {
        return;
    };
    let prefix = format!("{}.", name.to_string_lossy());
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let stale = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| {
            t.elapsed()
                .is_ok_and(|age| age > Duration::from_secs(86_400))
        });
        if stale && file.starts_with(&prefix) && file.ends_with(".part") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The file name a downloaded extract is kept under: the url's last part,
/// when that is a plain `.pbf` name.
fn download_name(url: &str) -> Option<&str> {
    url.rsplit('/')
        .next()
        .filter(|n| n.ends_with(".pbf") && !n.contains(['\\', ':']) && !n.starts_with('.'))
}

/// How often a download or bake reports where it is.
const REPORT_EVERY: Duration = Duration::from_millis(500);

/// Download sizes seen, by file name, so a plan can name one before the
/// extract is on disk.
const SIZES: &str = "sizes.json";

fn read_sizes(downloads: &Path) -> HashMap<String, u64> {
    std::fs::read(downloads.join(SIZES))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn remember_size(downloads: &Path, name: &str, bytes: u64) {
    let mut sizes = read_sizes(downloads);
    if sizes.get(name) == Some(&bytes) {
        return;
    }
    sizes.insert(name.to_string(), bytes);
    let _ = std::fs::create_dir_all(downloads);
    if let Ok(body) = serde_json::to_vec(&sizes) {
        let _ = crate::world_utils::replace_file_atomically(&downloads.join(SIZES), &body);
    }
}

/// The Content-Length a response declares, when it names a size.
fn content_length_header(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|&n| n > 0)
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // Per read, not per transfer: a country is hundreds of megabytes.
        .timeout(Duration::from_secs(120))
        .user_agent(crate::retrieve_data::OSM_USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// The Region Download extract: [`fetch_extract`] as the first 40 % of its
/// job, on the prewarm bar's 1-4 %.
fn download(url: &str, dest: &Path, cap: Option<u64>) -> Result<()> {
    fetch_extract(url, dest, (1, 1), (0.0, 40.0), None, Some((1.0, 4.0)), cap).map(drop)
}

fn dir_of(path: &Path) -> Result<&Path> {
    path.parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))
}

/// Bytes below which another connection is not worth opening.
const MIN_SEGMENT: u64 = 8 << 20;
/// Connections one extract is fetched over at most, within --max-downloads.
const MAX_SEGMENTS: usize = 8;

/// How many connections a download of `total` bytes gets: one per
/// [`MIN_SEGMENT`], at most `allowed` (--max-downloads) and
/// [`MAX_SEGMENTS`]; one when the server takes no ranges or names no size.
fn segment_count(total: u64, ranges: bool, allowed: usize) -> usize {
    if !ranges || total == 0 {
        return 1;
    }
    ((total / MIN_SEGMENT) as usize).clamp(1, allowed.clamp(1, MAX_SEGMENTS))
}

/// Inclusive `(start, end)` byte ranges splitting `total` into `n` parts.
fn segments(total: u64, n: usize) -> Vec<(u64, u64)> {
    let step = total.div_ceil(n.max(1) as u64).max(1);
    (0..total.div_ceil(step))
        .map(|i| (i * step, ((i + 1) * step).min(total) - 1))
        .collect()
}

/// The file a HEAD saw: where the link led, its size, and what names its version.
struct Remote {
    url: String,
    total: u64,
    version: Option<reqwest::header::HeaderValue>,
}

/// What `If-Range` takes of a HEAD's headers: a strong ETag, else Last-Modified.
fn version_header(headers: &reqwest::header::HeaderMap) -> Option<reqwest::header::HeaderValue> {
    use reqwest::header::{ETAG, LAST_MODIFIED};
    [ETAG, LAST_MODIFIED]
        .iter()
        .filter_map(|name| headers.get(name))
        .find(|v| !v.as_bytes().starts_with(b"W/"))
        .cloned()
}

/// Whether a 206's Content-Range is `span` of a file of `total` bytes.
fn is_span_of(content_range: &str, (start, end): (u64, u64), total: u64) -> bool {
    content_range
        .strip_prefix("bytes ")
        .and_then(|r| r.split_once('/'))
        .is_some_and(|(range, size)| {
            range == format!("{start}-{end}") && (size == "*" || size.parse() == Ok(total))
        })
}

/// Whether the file is whole blobs to its end, which a download cut short is not.
fn ends_on_a_blob(pbf: &Path) -> bool {
    std::fs::File::open(pbf).is_ok_and(|f| {
        let mut blobs = BlobReader::new(std::io::BufReader::new(f)).peekable();
        blobs.peek().is_some() && blobs.all(|b| b.is_ok())
    })
}

/// One connection's share of a download: `span` (or the whole file) into
/// `part` at its offset, counting into `done`. Stops early, without error,
/// once `quit` says so.
fn fetch_span(
    client: &reqwest::blocking::Client,
    remote: &Remote,
    part: &Path,
    span: Option<(u64, u64)>,
    done: &std::sync::atomic::AtomicU64,
    quit: &dyn Fn() -> bool,
) -> Result<()> {
    use reqwest::header::{CONTENT_RANGE, IF_RANGE, RANGE};
    use std::io::{Seek, SeekFrom};
    let _permit = crate::net::request_permit();
    let source = remote.url.as_str();
    let mut req = client.get(source);
    if let Some((start, end)) = span {
        req = req.header(RANGE, format!("bytes={start}-{end}"));
        // A file replaced since the HEAD then answers 200, not a range of another version.
        if let Some(version) = &remote.version {
            req = req.header(IF_RANGE, version);
        }
    }
    let mut resp = req
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{source}: {e}"))?;
    if let Some(span) = span {
        let range = resp.headers().get(CONTENT_RANGE);
        let range = range.and_then(|v| v.to_str().ok()).unwrap_or_default();
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT
            || !is_span_of(range, span, remote.total)
        {
            return Err(format!(
                "{source}: the server did not send the byte range asked for; the file may have changed"
            ));
        }
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .open(part)
        .map_err(|e| format!("{}: {e}", part.display()))?;
    out.seek(SeekFrom::Start(span.map_or(0, |s| s.0)))
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    let mut got = 0u64;
    while !quit() {
        let k = resp.read(&mut buf).map_err(|e| format!("{source}: {e}"))?;
        if k == 0 {
            // A dropped connection can end as a clean EOF.
            let want = span.map_or(0, |(a, b)| b - a + 1);
            if want > 0 && got < want {
                return Err(format!(
                    "{source}: connection dropped at {got} of {want} bytes"
                ));
            }
            break;
        }
        out.write_all(&buf[..k]).map_err(|e| e.to_string())?;
        got += k as u64;
        done.fetch_add(k as u64, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(())
}

/// Streams `url` to this process's `.part` beside `dest` over byte-range connections, one --max-downloads permit
/// each, then renames it into place; reports progress for `item` across `span` (and `bar`).
/// `Ok(false)` when `cancel` stopped it; a HEAD size past `cap` is refused.
pub fn fetch_extract(
    url: &str,
    dest: &Path,
    item: (usize, usize),
    span: (f64, f64),
    cancel: Option<&std::sync::atomic::AtomicBool>,
    bar: Option<(f64, f64)>,
    cap: Option<u64>,
) -> Result<bool> {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    crate::net::ensure_online("OpenStreetMap extract (.pbf)")?;
    let client = client()?;
    // One HEAD names the size, the range support and the file a `-latest`
    // link redirects to, so every connection reads the same version.
    let head = {
        let _permit = crate::net::request_permit();
        client
            .head(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("{url}: {e}"))?
    };
    let total = content_length_header(head.headers()).unwrap_or(0);
    if let Some(cap) = cap {
        check_extract_size(&extract_label(dest), total, cap)?;
    }
    println!("Downloading {url}");
    let ranges = head
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .is_some_and(|v| v.as_bytes() == b"bytes");
    let remote = Remote {
        url: head.url().to_string(),
        total,
        version: version_header(head.headers()),
    };
    let n = segment_count(total, ranges, crate::net::max_requests());
    let dir = dir_of(dest)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if total > 0 {
        if let Some(name) = dest.file_name() {
            remember_size(dir, &name.to_string_lossy(), total);
        }
    }
    let label = extract_label(dest);
    let (index, items) = (item.0.max(1) - 1, item.1.max(1));
    let report = |done: u64, rate_bps: f64| {
        let frac = if total > 0 {
            done as f64 / total as f64
        } else {
            0.0
        };
        let percent = span.0 + (span.1 - span.0) * frac;
        let (progress, message) = match bar {
            Some((from, to)) => (from + (to - from) * frac, String::new()),
            None => (
                percent,
                format!(
                    "Downloading {label} ({}/{items}): {:.0}%",
                    index + 1,
                    100.0 * frac
                ),
            ),
        };
        crate::progress::emit_gui_transfer(
            progress,
            &message,
            &Transfer {
                stage: Stage::Download,
                name: label.clone(),
                item: index + 1,
                items,
                done_bytes: done,
                total_bytes: total,
                rate_bps,
                downloads: n as u32,
                percent,
                ..Transfer::default()
            },
        );
    };
    report(0, 0.0);

    remove_stale_parts(dest);
    let part = part_path(dest);
    let file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    if total > 0 {
        file.set_len(total).map_err(|e| e.to_string())?;
    }
    let done = AtomicU64::new(0);
    let failed = AtomicBool::new(false);
    let stopped = || cancel.is_some_and(|c| c.load(Ordering::Acquire));
    let quit = || stopped() || failed.load(Ordering::Acquire);
    let spans: Vec<Option<(u64, u64)>> = if n > 1 {
        segments(total, n).into_iter().map(Some).collect()
    } else {
        vec![None]
    };
    let fetched: Result<()> = std::thread::scope(|scope| {
        let workers: Vec<_> = spans
            .iter()
            .map(|&span| {
                let (client, remote, part, done, failed, quit) =
                    (&client, &remote, &part, &done, &failed, &quit);
                scope.spawn(move || {
                    let r = fetch_span(client, remote, part, span, done, quit);
                    // A failed connection stops the others.
                    if r.is_err() {
                        failed.store(true, Ordering::Release);
                    }
                    r
                })
            })
            .collect();
        let started = Instant::now();
        let mut rate = RateMeter::default();
        let mut last = Instant::now();
        while !workers.iter().all(|w| w.is_finished()) {
            std::thread::sleep(Duration::from_millis(50));
            if last.elapsed() >= REPORT_EVERY {
                last = Instant::now();
                let d = done.load(Ordering::Relaxed);
                report(d, rate.add(started.elapsed().as_secs_f64(), d));
            }
        }
        let d = done.load(Ordering::Relaxed);
        report(d, rate.add(started.elapsed().as_secs_f64(), d));
        // The scope joins any the first error leaves.
        workers.into_iter().try_for_each(|w| {
            w.join()
                .unwrap_or_else(|_| Err("a download thread panicked".into()))
        })
    });
    let outcome = if stopped() {
        Ok(false)
    } else {
        fetched.and_then(|()| {
            file.sync_all().map_err(|e| e.to_string())?;
            let d = done.load(Ordering::Relaxed);
            if total > 0 && d != total {
                return Err(format!("{url}: got {d} of {total} bytes"));
            }
            // No size was declared to hold it to.
            if total == 0 && !ends_on_a_blob(&part) {
                return Err(format!("{url}: the download ended early"));
            }
            Ok(true)
        })
    };
    // Closed before the rename, which Windows refuses on an open file.
    drop(file);
    match outcome {
        Ok(true) => std::fs::rename(&part, dest)
            .map(|()| true)
            .map_err(|e| e.to_string()),
        other => {
            let _ = std::fs::remove_file(&part);
            other
        }
    }
}

/// The Geofabrik index, from a cached copy younger than a week (any age offline).
fn fetch_index() -> Result<serde_json::Value> {
    let path = cache_root()?.join("geofabrik-index.json");
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    if age.is_some_and(|a| a < INDEX_MAX_AGE || crate::net::offline()) {
        let cached = std::fs::read(&path).ok();
        if let Some(v) = cached.and_then(|b| serde_json::from_slice(&b).ok()) {
            return Ok(v);
        }
    }
    crate::net::ensure_online("Geofabrik extract index")?;
    let body = {
        let _permit = crate::net::request_permit();
        client()?
            .get(GEOFABRIK_INDEX_URL)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| format!("Geofabrik index unreachable: {e}"))?
    };
    let v = serde_json::from_slice(&body).map_err(|e| format!("bad Geofabrik index: {e}"))?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = crate::world_utils::replace_file_atomically(&path, &body);
    Ok(v)
}

/// The .pbf url of the smallest extract whose border holds the whole selection. A bbox
/// test would lie (a country's bbox spans its neighbours), so a 5x5 grid of points across
/// the selection is tested against the border polygon, ranked by polygon area.
fn geofabrik_url(index: &serde_json::Value, want: &E7Box) -> Result<String> {
    geofabrik_feature(index, want).and_then(|f| {
        f["properties"]["urls"]["pbf"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "bad Geofabrik index".to_string())
    })
}

/// The index entry of the smallest extract holding the selection.
fn geofabrik_feature<'a>(
    index: &'a serde_json::Value,
    want: &E7Box,
) -> Result<&'a serde_json::Value> {
    const N: i64 = 5;
    let points: Vec<(f64, f64)> = (0..N * N)
        .map(|k| {
            let (i, j) = (k / N, k % N);
            let lon = want.min_lon + (want.max_lon - want.min_lon) * j / (N - 1);
            let lat = want.min_lat + (want.max_lat - want.min_lat) * i / (N - 1);
            (lon as f64 / 1e7, lat as f64 / 1e7)
        })
        .collect();
    let features = index["features"].as_array().ok_or("bad Geofabrik index")?;
    features
        .iter()
        .filter_map(|f| {
            f["properties"]["urls"]["pbf"].as_str()?;
            let rings = rings(&f["geometry"]);
            (!rings.is_empty() && points.iter().all(|&(x, y)| in_rings(x, y, &rings)))
                .then(|| (rings_area(&rings), f))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
        .ok_or_else(|| {
            "no single Geofabrik extract holds this selection; pass a .pbf file with --osm-pbf"
                .to_string()
        })
}

pub(crate) type Ring = Vec<(f64, f64)>;

/// A GeoJSON (Multi)Polygon as a flat ring list; even-odd needs no outer/hole bookkeeping.
pub(crate) fn rings(geom: &serde_json::Value) -> Vec<Ring> {
    let ring = |r: &serde_json::Value| -> Ring {
        r.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
            .collect()
    };
    let polys: Vec<&serde_json::Value> = match geom["type"].as_str() {
        Some("Polygon") => vec![&geom["coordinates"]],
        Some("MultiPolygon") => geom["coordinates"]
            .as_array()
            .into_iter()
            .flatten()
            .collect(),
        _ => Vec::new(),
    };
    polys
        .into_iter()
        .flat_map(|p| p.as_array().into_iter().flatten().map(ring))
        .filter(|r| r.len() > 2)
        .collect()
}

pub(crate) fn in_rings(x: f64, y: f64, rings: &[Ring]) -> bool {
    let mut inside = false;
    for ring in rings {
        let mut j = ring.len() - 1;
        for i in 0..ring.len() {
            let ((xi, yi), (xj, yj)) = (ring[i], ring[j]);
            if (yi > y) != (yj > y) && x < xj + (y - yj) * (xi - xj) / (yi - yj) {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

/// Sum of ring areas, deg². A ranking key: a bbox area would rank France (overseas
/// territories) above Europe.
fn rings_area(rings: &[Ring]) -> f64 {
    rings
        .iter()
        .map(|r| {
            let mut a = 0.0;
            let mut j = r.len() - 1;
            for i in 0..r.len() {
                a += (r[j].0 + r[i].0) * (r[j].1 - r[i].1);
                j = i;
            }
            a.abs() / 2.0
        })
        .sum()
}

// ── the cut ───────────────────────────────────────────────────────────────────

/// A lat/lon box in degrees x 1e7, the .pbf's own unit, so containment is exact.
#[derive(Clone, Copy, Debug, PartialEq)]
struct E7Box {
    min_lat: i64,
    min_lon: i64,
    max_lat: i64,
    max_lon: i64,
}

impl E7Box {
    fn around(bbox: &LLBBox, pad_m: f64) -> Self {
        let mid = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let dlat = pad_m / 111_320.0;
        let dlon = pad_m / (111_320.0 * mid.to_radians().cos().max(0.01));
        E7Box {
            min_lat: ((bbox.min().lat() - dlat) * 1e7).floor().max(-90e7) as i64,
            min_lon: ((bbox.min().lng() - dlon) * 1e7).floor().max(-180e7) as i64,
            max_lat: ((bbox.max().lat() + dlat) * 1e7).ceil().min(90e7) as i64,
            max_lon: ((bbox.max().lng() + dlon) * 1e7).ceil().min(180e7) as i64,
        }
    }

    fn point(lat: i64, lon: i64) -> Self {
        E7Box {
            min_lat: lat,
            min_lon: lon,
            max_lat: lat,
            max_lon: lon,
        }
    }

    fn union(self, o: Self) -> Self {
        E7Box {
            min_lat: self.min_lat.min(o.min_lat),
            min_lon: self.min_lon.min(o.min_lon),
            max_lat: self.max_lat.max(o.max_lat),
            max_lon: self.max_lon.max(o.max_lon),
        }
    }

    fn intersects(&self, o: &Self) -> bool {
        self.min_lat <= o.max_lat
            && o.min_lat <= self.max_lat
            && self.min_lon <= o.max_lon
            && o.min_lon <= self.max_lon
    }

    fn contains(&self, o: &Self) -> bool {
        self.min_lat <= o.min_lat
            && self.min_lon <= o.min_lon
            && self.max_lat >= o.max_lat
            && self.max_lon >= o.max_lon
    }

    fn holds(&self, lat: i64, lon: i64) -> bool {
        self.contains(&E7Box::point(lat, lon))
    }

    fn area(&self) -> i128 {
        i128::from(self.max_lat - self.min_lat) * i128::from(self.max_lon - self.min_lon)
    }

    /// The area in square degrees, the unit of [`rings_area`].
    fn area_deg2(&self) -> f64 {
        self.area() as f64 / 1e14
    }

    fn file_name(&self) -> String {
        format!(
            "{}_{}_{}_{}.json.zst",
            self.min_lat, self.min_lon, self.max_lat, self.max_lon
        )
    }

    fn from_file_name(name: &str) -> Option<Self> {
        let v: Vec<i64> = name
            .strip_suffix(".json.zst")?
            .split('_')
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        match v[..] {
            [min_lat, min_lon, max_lat, max_lon] => Some(E7Box {
                min_lat,
                min_lon,
                max_lat,
                max_lon,
            }),
            _ => None,
        }
    }
}

fn e7(deg: f64) -> i64 {
    (deg * 1e7).round() as i64
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Node,
    Way,
    Relation,
}

/// Whether the Overpass query (see `fetch_data_from_overpass`) asks for an element of `kind`
/// with these tags: a candidate (a key it names, `place` and rail stops on nodes, `type=building`
/// on relations, any tag at all on a way) that the exclusions do not take out again.
fn wanted<'a>(kind: Kind, any_tag: bool, tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    const ANY_VALUE: &[&str] = &[
        "building",
        "building:part",
        "highway",
        "leisure",
        "amenity",
        "tourism",
        "bridge",
        "railway",
        "roller_coaster",
        "aerialway",
        "attraction",
        "barrier",
        "entrance",
        "door",
        "power",
        "historic",
        "emergency",
        "advertising",
        "man_made",
        "aeroway",
        "3dmr",
        "shop",
        "office",
        "landuse",
        "natural",
        "water",
        "waterway",
    ];
    let excluded = tag("landuse") == Some("salt_pond")
        || matches!(tag("natural"), Some("coastline" | "bay" | "strait"))
        || matches!(tag("water"), Some("bay" | "ocean" | "sea"))
        || tag("tidal") == Some("yes")
        || tag("waterway") == Some("tidal_channel")
        || matches!(
            tag("place"),
            Some("ocean" | "sea" | "bay" | "strait" | "sound" | "fjord")
        );
    if excluded {
        return false;
    }
    let rail_stop = || {
        matches!(tag("public_transport"), Some("stop_position" | "station"))
            && ["train", "tram", "subway", "light_rail"]
                .iter()
                .any(|k| tag(k) == Some("yes"))
    };
    let place = || {
        matches!(
            tag("place"),
            Some(
                "city"
                    | "town"
                    | "borough"
                    | "suburb"
                    | "village"
                    | "quarter"
                    | "neighbourhood"
                    | "hamlet"
            )
        )
    };
    ANY_VALUE.iter().any(|k| tag(k).is_some())
        || match kind {
            Kind::Way => any_tag,
            Kind::Node => rail_stop() || place() || tag("raceway").is_some(),
            Kind::Relation => tag("type") == Some("building"),
        }
}

fn wanted_map(kind: Kind, tags: &Option<HashMap<String, String>>) -> bool {
    wanted(kind, tags.as_ref().is_some_and(|t| !t.is_empty()), |k| {
        tags.as_ref()?.get(k).map(String::as_str)
    })
}

/// What the Overpass query returns for `b`, from elements that hold at least that:
/// every way whose extent meets `b` (an enclosing lake too, as with the tile archive),
/// every wanted relation whose member ways' extent does, with all of its member ways,
/// the nodes of those ways, and the wanted tagged nodes inside `b`. A node keeps its tags
/// only as such a node. Sorted by id, so the bake and any cut of it are deterministic,
/// and a cut of a bake equals the cut of the extract.
fn cut(elements: &[OsmElement], b: E7Box) -> OsmData {
    let mut nodes: HashMap<u64, &OsmElement> = HashMap::new();
    let mut ways: HashMap<u64, &OsmElement> = HashMap::new();
    let mut rels: Vec<&OsmElement> = Vec::new();
    for e in elements {
        match e.r#type.as_str() {
            "node" => {
                nodes.entry(e.id).or_insert(e);
            }
            "way" => {
                ways.entry(e.id).or_insert(e);
            }
            "relation" => rels.push(e),
            _ => {}
        }
    }
    let at = |id: &u64| {
        let n = nodes.get(id)?;
        Some((e7(n.lat?), e7(n.lon?)))
    };
    let extent: HashMap<u64, E7Box> = ways
        .iter()
        .filter_map(|(&id, w)| {
            let e = w
                .nodes
                .iter()
                .flatten()
                .filter_map(at)
                .map(|(lat, lon)| E7Box::point(lat, lon))
                .reduce(E7Box::union)?;
            Some((id, e))
        })
        .collect();

    let mut keep_ways: HashSet<u64> = extent
        .iter()
        .filter(|(id, e)| e.intersects(&b) && wanted_map(Kind::Way, &ways[id].tags))
        .map(|(&id, _)| id)
        .collect();
    let mut keep_rels: Vec<&OsmElement> = rels
        .into_iter()
        .filter(|r| wanted_map(Kind::Relation, &r.tags))
        .filter(|r| {
            r.members
                .iter()
                .filter(|m| m.r#type == "way")
                .filter_map(|m| extent.get(&m.r#ref).copied())
                .reduce(E7Box::union)
                .is_some_and(|e| e.intersects(&b))
        })
        .collect();
    keep_rels.sort_unstable_by_key(|r| r.id);
    keep_rels.dedup_by_key(|r| r.id);
    for r in &keep_rels {
        keep_ways.extend(
            r.members
                .iter()
                .filter(|m| m.r#type == "way" && ways.contains_key(&m.r#ref))
                .map(|m| m.r#ref),
        );
    }

    let tagged_inside = |n: &OsmElement| {
        wanted_map(Kind::Node, &n.tags)
            && matches!((n.lat, n.lon), (Some(lat), Some(lon)) if b.holds(e7(lat), e7(lon)))
    };
    let mut keep_nodes: HashSet<u64> = nodes
        .values()
        .filter(|n| tagged_inside(n))
        .map(|n| n.id)
        .collect();
    for id in &keep_ways {
        keep_nodes.extend(
            ways[id]
                .nodes
                .iter()
                .flatten()
                .filter(|n| nodes.contains_key(n)),
        );
    }

    let mut node_ids: Vec<u64> = keep_nodes.into_iter().collect();
    node_ids.sort_unstable();
    let mut way_ids: Vec<u64> = keep_ways.into_iter().collect();
    way_ids.sort_unstable();
    let mut out: Vec<OsmElement> = Vec::with_capacity(node_ids.len() + way_ids.len());
    out.extend(node_ids.iter().map(|id| {
        let n = nodes[id];
        OsmElement {
            r#type: "node".into(),
            id: n.id,
            lat: n.lat,
            lon: n.lon,
            nodes: None,
            tags: n.tags.clone().filter(|_| tagged_inside(n)),
            members: Vec::new(),
        }
    }));
    out.extend(way_ids.iter().map(|id| ways[id].clone()));
    out.extend(keep_rels.into_iter().cloned());
    OsmData::from_elements(out)
}

/// The extract, read for pass `pass` (1-based) of a bake, reporting how far
/// through the file the read is.
struct Counted {
    inner: std::fs::File,
    read: u64,
    total: u64,
    pass: u32,
    label: String,
    last: Instant,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if self.last.elapsed() >= REPORT_EVERY || n == 0 {
            self.last = Instant::now();
            let frac = (f64::from(self.pass - 1) + self.read as f64 / self.total.max(1) as f64)
                / BAKE_PASSES;
            report_bake(&self.label, Stage::Bake, frac, self.read, self.total);
        }
        Ok(n)
    }
}

/// Runs `f` over every data block of the extract on the rayon pool, so `--threads` and
/// the piece's thread share decide how many blocks decode at once. `pass` numbers the
/// read for the progress bar.
fn par_blocks<T: Send>(
    pbf: &Path,
    pass: u32,
    f: impl Fn(&PrimitiveBlock) -> T + Sync + Send,
) -> Result<Vec<T>> {
    let file = std::fs::File::open(pbf).map_err(|e| format!("{}: {e}", pbf.display()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let reader = BlobReader::new(std::io::BufReader::new(Counted {
        inner: file,
        read: 0,
        total,
        pass,
        label: extract_label(pbf),
        last: Instant::now(),
    }));
    reader
        .par_bridge()
        .filter_map(|blob| {
            let decoded = blob.and_then(|b| match b.decode()? {
                BlobDecode::OsmData(block) => Ok(Some(f(&block))),
                _ => Ok(None),
            });
            decoded.transpose()
        })
        .collect::<std::result::Result<Vec<T>, _>>()
        .map_err(|e| format!("{}: {e}", pbf.display()))
}

fn tag_map<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> HashMap<String, String> {
    tags.map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn node_element(id: i64, lat: i32, lon: i32, tags: Option<HashMap<String, String>>) -> OsmElement {
    OsmElement {
        r#type: "node".into(),
        id: id as u64,
        lat: Some(f64::from(lat) / 1e7),
        lon: Some(f64::from(lon) / 1e7),
        nodes: None,
        tags,
        members: Vec::new(),
    }
}

/// [`cut`] of the extract. The first pass keeps every node's location (16 bytes a node, as
/// osmium's own index) and the wanted relations; the second the ways that meet the area,
/// and only the extent of other relation members; a third, when needed, the members of the
/// relations that meet the area.
fn cut_pbf(pbf: &Path, want: E7Box) -> Result<OsmData> {
    type Pass1 = (Vec<(i64, i32, i32)>, Vec<OsmElement>, Vec<OsmElement>);
    let pass1: Vec<Pass1> = par_blocks(pbf, 1, |block| {
        // A dense block holds 8000 nodes: reserved, then trimmed, it never doubles past that.
        let (mut locs, mut tagged, mut rels) = (Vec::with_capacity(8000), Vec::new(), Vec::new());
        let mut node = |id: i64, lat: i32, lon: i32, tags: Vec<(&str, &str)>| {
            locs.push((id, lat, lon));
            let find = |k: &str| tags.iter().find(|t| t.0 == k).map(|t| t.1);
            if want.holds(lat.into(), lon.into()) && wanted(Kind::Node, !tags.is_empty(), find) {
                tagged.push(node_element(id, lat, lon, Some(tag_map(tags.into_iter()))));
            }
        };
        for el in block.elements() {
            match el {
                Element::Node(n) => node(
                    n.id(),
                    n.decimicro_lat(),
                    n.decimicro_lon(),
                    n.tags().collect(),
                ),
                Element::DenseNode(n) => node(
                    n.id(),
                    n.decimicro_lat(),
                    n.decimicro_lon(),
                    n.tags().collect(),
                ),
                Element::Relation(r) => {
                    let tags: Vec<(&str, &str)> = r.tags().collect();
                    if !wanted(Kind::Relation, !tags.is_empty(), |k| {
                        tags.iter().find(|t| t.0 == k).map(|t| t.1)
                    }) {
                        continue;
                    }
                    rels.push(OsmElement {
                        r#type: "relation".into(),
                        id: r.id() as u64,
                        lat: None,
                        lon: None,
                        nodes: None,
                        tags: Some(tag_map(tags.into_iter())),
                        members: r
                            .members()
                            .map(|m| OsmMember {
                                r#type: match m.member_type {
                                    RelMemberType::Node => "node",
                                    RelMemberType::Way => "way",
                                    RelMemberType::Relation => "relation",
                                }
                                .into(),
                                r#ref: m.member_id as u64,
                                role: m.role().unwrap_or_default().to_string(),
                            })
                            .collect(),
                    });
                }
                Element::Way(_) => {}
            }
        }
        locs.shrink_to_fit();
        (locs, tagged, rels)
    })?;
    // Sized up front: growing by doubling would hold up to twice the index at once.
    let mut locs: Vec<(i64, i32, i32)> = Vec::with_capacity(pass1.iter().map(|p| p.0.len()).sum());
    let (mut tagged, mut rels): (Vec<OsmElement>, Vec<OsmElement>) = (Vec::new(), Vec::new());
    for (l, t, r) in pass1 {
        locs.extend(l);
        tagged.extend(t);
        rels.extend(r);
    }
    locs.par_sort_unstable_by_key(|n| n.0);
    let loc = |id: i64| {
        locs.binary_search_by_key(&id, |n| n.0)
            .ok()
            .map(|i| (locs[i].1, locs[i].2))
    };
    let extent = |refs: &[i64]| {
        refs.iter()
            .filter_map(|&id| loc(id))
            .map(|(lat, lon)| E7Box::point(lat.into(), lon.into()))
            .reduce(E7Box::union)
    };
    let way_ids = |r: &OsmElement| -> Vec<i64> {
        r.members
            .iter()
            .filter(|m| m.r#type == "way")
            .map(|m| m.r#ref as i64)
            .collect()
    };
    let members: HashSet<i64> = rels.iter().flat_map(way_ids).collect();

    type Pass2 = (Vec<OsmElement>, Vec<(i64, E7Box)>);
    let pass2: Vec<Pass2> = par_blocks(pbf, 2, |block| {
        let (mut full, mut outside) = (Vec::new(), Vec::new());
        for el in block.elements() {
            let Element::Way(w) = el else { continue };
            let refs: Vec<i64> = w.refs().collect();
            let candidate = wanted(Kind::Way, w.tags().next().is_some(), |k| {
                w.tags().find(|t| t.0 == k).map(|t| t.1)
            });
            match extent(&refs) {
                Some(e) if candidate && e.intersects(&want) => full.push(way_element(&w, refs)),
                Some(e) if members.contains(&w.id()) => outside.push((w.id(), e)),
                _ => {}
            }
        }
        (full, outside)
    })?;
    let mut ways: Vec<OsmElement> = Vec::new();
    let mut outside: HashMap<i64, E7Box> = HashMap::new();
    for (f, o) in pass2 {
        ways.extend(f);
        outside.extend(o);
    }
    let inside: HashSet<i64> = ways.iter().map(|w| w.id as i64).collect();
    // As in the cut: a member meeting the area, or members that together enclose it.
    rels.retain(|r| {
        let ids = way_ids(r);
        ids.iter().any(|id| inside.contains(id))
            || ids
                .iter()
                .filter_map(|id| outside.get(id).copied())
                .reduce(E7Box::union)
                .is_some_and(|e| e.intersects(&want))
    });
    let missing: HashSet<i64> = rels
        .iter()
        .flat_map(way_ids)
        .filter(|id| outside.contains_key(id))
        .collect();
    drop(outside);
    if !missing.is_empty() {
        let pass3: Vec<Vec<OsmElement>> = par_blocks(pbf, 3, |block| {
            block
                .elements()
                .filter_map(|el| match el {
                    Element::Way(w) if missing.contains(&w.id()) => {
                        Some(way_element(&w, w.refs().collect()))
                    }
                    _ => None,
                })
                .collect()
        })?;
        ways.extend(pass3.into_iter().flatten());
    }

    // Tagged nodes first: the cut keeps the first copy of a node.
    let mut elements = tagged;
    let vertices: HashSet<i64> = ways
        .iter()
        .flat_map(|w| w.nodes.iter().flatten())
        .map(|&id| id as i64)
        .collect();
    for id in vertices {
        if let Some((lat, lon)) = loc(id) {
            elements.push(node_element(id, lat, lon, None));
        }
    }
    elements.extend(ways);
    elements.extend(rels);
    Ok(cut(&elements, want))
}

fn way_element(w: &osmpbf::Way, refs: Vec<i64>) -> OsmElement {
    let tags = tag_map(w.tags());
    OsmElement {
        r#type: "way".into(),
        id: w.id() as u64,
        lat: None,
        lon: None,
        nodes: Some(refs.into_iter().map(|r| r as u64).collect()),
        tags: (!tags.is_empty()).then_some(tags),
        members: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_only_for_geofabrik_online() {
        let src = |spec: &str| Source {
            spec: spec.into(),
            url: None,
            pad_m: 0.0,
        };
        assert!(fallback_allowed(&src(GEOFABRIK), false));
        assert!(!fallback_allowed(&src(GEOFABRIK), true));
        assert!(!fallback_allowed(&src("berlin.osm.pbf"), false));
    }

    #[test]
    fn an_empty_cut_falls_back_only_for_geofabrik() {
        // Held by the tests that switch --offline on, which changes the answer.
        let _net = crate::net::PERMIT_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _floor = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let src = |spec: &str| Source {
            spec: spec.into(),
            url: None,
            pad_m: 0.0,
        };
        assert!(no_data(&src(GEOFABRIK)).is_err());
        assert!(no_data(&src("berlin.osm.pbf")).is_ok());
    }

    #[test]
    fn downloads_are_kept_per_url_and_a_picked_one_is_capped() {
        let cache = Path::new("/c");
        let us = download_path(cache, "https://d/north-america/us/georgia-latest.osm.pbf");
        let country = download_path(cache, "https://d/asia/georgia-latest.osm.pbf").unwrap();
        assert_ne!(us.as_deref(), Some(country.as_path()));
        assert!(country.ends_with("georgia-latest.osm.pbf"));
        assert_eq!(extract_label(&country), "georgia");
        assert!(download_path(cache, "https://d/index.html").is_none());

        assert!(check_extract_size("liechtenstein", 3_463_268, MAX_PICKED_EXTRACT_BYTES).is_ok());
        let e = check_extract_size("germany", 4_400_000_000, MAX_PICKED_EXTRACT_BYTES).unwrap_err();
        assert!(e.contains("germany") && e.contains("4.4 GB"), "{e}");
    }

    #[test]
    fn a_short_extract_is_fetched_again_and_parts_are_per_process() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("liechtenstein-latest.osm.pbf");
        assert!(!downloaded_whole(&dest));
        std::fs::write(&dest, [0u8; 10]).unwrap();
        // No size on record: taken as it is.
        assert!(downloaded_whole(&dest));
        remember_size(tmp.path(), "liechtenstein-latest.osm.pbf", 10);
        assert!(downloaded_whole(&dest));
        remember_size(tmp.path(), "liechtenstein-latest.osm.pbf", 12);
        assert!(!downloaded_whole(&dest));
        let part = part_path(&dest);
        let name = part.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            name,
            format!("liechtenstein-latest.osm.pbf.{}.part", std::process::id())
        );
        // A part nothing wrote to for a day goes; a fresh one stays.
        let old = tmp.path().join("liechtenstein-latest.osm.pbf.1.part");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&part, b"x").unwrap();
        let day_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
        remove_stale_parts(&dest);
        assert!(!old.exists() && part.exists() && dest.exists());
    }

    #[test]
    fn raceway_nodes_are_wanted_like_overpass_asks() {
        let tags = [("raceway", "start_line")];
        let find = |k: &str| tags.iter().find(|t| t.0 == k).map(|t| t.1);
        assert!(wanted(Kind::Node, true, find));
        assert!(!wanted(Kind::Node, true, |k| (k == "name").then_some("x")));
    }

    #[test]
    fn a_download_splits_into_whole_ranges_within_the_cap() {
        let mb = 1u64 << 20;
        // No ranges or no size: one connection.
        assert_eq!(segment_count(100 * mb, false, 16), 1);
        assert_eq!(segment_count(0, true, 16), 1);
        // One per 8 MB, under --max-downloads and the cap of 8.
        assert_eq!(segment_count(3 * mb, true, 16), 1);
        assert_eq!(segment_count(48 * mb, true, 16), 6);
        assert_eq!(segment_count(800 * mb, true, 16), 8);
        assert_eq!(segment_count(800 * mb, true, 2), 2);
        assert_eq!(segment_count(800 * mb, true, 0), 1);
        // The ranges cover every byte once.
        for (total, n) in [(47_647_983u64, 5usize), (10, 3), (7, 8), (1, 1), (16, 4)] {
            let s = segments(total, n);
            assert!(s.len() <= n && !s.is_empty(), "{total} {n}: {s:?}");
            assert_eq!(s[0].0, 0);
            assert_eq!(s.last().unwrap().1, total - 1);
            assert!(s.windows(2).all(|w| w[1].0 == w[0].1 + 1), "{s:?}");
        }
    }

    #[test]
    fn a_head_size_comes_from_the_header() {
        use reqwest::header::{HeaderMap, HeaderValue, CONTENT_LENGTH};
        let mut h = HeaderMap::new();
        assert_eq!(content_length_header(&h), None);
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("3463268"));
        assert_eq!(content_length_header(&h), Some(3_463_268));
        h.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
        assert_eq!(
            content_length_header(&h),
            None,
            "no size, not an empty extract"
        );
    }

    #[test]
    fn ranges_are_held_to_the_version_the_head_saw() {
        use reqwest::header::{HeaderMap, HeaderValue, ETAG, LAST_MODIFIED};
        let mut h = HeaderMap::new();
        assert_eq!(version_header(&h), None);
        let date = HeaderValue::from_static("Wed, 11 Nov 2026 21:20:00 GMT");
        h.insert(LAST_MODIFIED, date.clone());
        // A weak ETag is no validator for If-Range.
        h.insert(ETAG, HeaderValue::from_static("W/\"abc\""));
        assert_eq!(version_header(&h), Some(date));
        let etag = HeaderValue::from_static("\"abc\"");
        h.insert(ETAG, etag.clone());
        assert_eq!(version_header(&h), Some(etag));

        assert!(is_span_of("bytes 0-99/1000", (0, 99), 1000));
        assert!(is_span_of("bytes 0-99/*", (0, 99), 1000));
        // Another file's size, another range, or no header at all.
        assert!(!is_span_of("bytes 0-99/1004", (0, 99), 1000));
        assert!(!is_span_of("bytes 0-49/1000", (0, 99), 1000));
        assert!(!is_span_of("", (0, 99), 1000));
    }

    #[test]
    fn an_extract_cut_short_does_not_end_on_a_blob() {
        let whole =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/osmpbf-test.osm.pbf");
        assert!(ends_on_a_blob(&whole));
        let bytes = std::fs::read(&whole).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cut = tmp.path().join("cut.osm.pbf");
        std::fs::write(&cut, &bytes[..bytes.len() - 7]).unwrap();
        assert!(!ends_on_a_blob(&cut));
        std::fs::write(&cut, b"").unwrap();
        assert!(!ends_on_a_blob(&cut));
    }

    fn node(id: u64, lat: f64, lon: f64, tags: &[(&str, &str)]) -> OsmElement {
        OsmElement {
            r#type: "node".into(),
            id,
            lat: Some(lat),
            lon: Some(lon),
            nodes: None,
            tags: (!tags.is_empty()).then(|| {
                tags.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            }),
            members: Vec::new(),
        }
    }

    fn way(id: u64, nodes: &[u64], tags: &[(&str, &str)]) -> OsmElement {
        OsmElement {
            r#type: "way".into(),
            id,
            lat: None,
            lon: None,
            nodes: Some(nodes.to_vec()),
            tags: Some(
                tags.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            members: Vec::new(),
        }
    }

    fn ids(d: &OsmData, kind: &str) -> Vec<u64> {
        d.elements()
            .iter()
            .filter(|e| e.r#type == kind)
            .map(|e| e.id)
            .collect()
    }

    fn bbox(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> E7Box {
        E7Box::around(
            &LLBBox::new(min_lat, min_lon, max_lat, max_lon).unwrap(),
            0.0,
        )
    }

    #[test]
    fn cut_keeps_what_overpass_would_and_a_cut_of_a_cut_agrees() {
        let lake = way(20, &[1, 2, 3, 4, 1], &[("natural", "water")]);
        let elements = vec![
            // A lake whose corners are all outside the area, around it.
            node(1, 0.0, 0.0, &[]),
            node(2, 0.0, 1.0, &[]),
            node(3, 1.0, 1.0, &[]),
            node(4, 1.0, 0.0, &[]),
            lake,
            // A road crossing into the area, and a bench in it; a name-only node is not asked for.
            node(5, 0.5, 0.5, &[("highway", "crossing")]),
            node(6, 0.6, 0.9, &[]),
            way(21, &[5, 6], &[("highway", "residential")]),
            node(7, 0.51, 0.51, &[("amenity", "bench")]),
            node(8, 0.52, 0.52, &[("name", "x")]),
            // Far away: a building, and a route relation (not asked for) holding it.
            node(9, 5.0, 5.0, &[]),
            node(10, 5.0, 5.1, &[]),
            way(22, &[9, 10], &[("building", "yes")]),
            OsmElement {
                r#type: "relation".into(),
                id: 30,
                lat: None,
                lon: None,
                nodes: None,
                tags: Some([("route".to_string(), "bus".to_string())].into()),
                members: vec![
                    OsmMember {
                        r#type: "way".into(),
                        r#ref: 21,
                        role: String::new(),
                    },
                    OsmMember {
                        r#type: "way".into(),
                        r#ref: 22,
                        role: String::new(),
                    },
                ],
            },
        ];
        let big = bbox(0.4, 0.4, 0.6, 0.6);
        let d = cut(&elements, big);
        assert_eq!(ids(&d, "way"), vec![20, 21]);
        assert_eq!(ids(&d, "node"), vec![1, 2, 3, 4, 5, 6, 7]);
        assert!(ids(&d, "relation").is_empty());
        let crossing = d.elements().iter().find(|e| e.id == 5).unwrap();
        assert!(crossing.tags.is_some());

        // A smaller area inside: from the first cut, or from the source, the same answer.
        let small = bbox(0.505, 0.505, 0.515, 0.515);
        let twice = cut(d.elements(), small);
        let once = cut(&elements, small);
        let key = |d: &OsmData| -> Vec<(String, u64, bool)> {
            d.elements()
                .iter()
                .map(|e| (e.r#type.clone(), e.id, e.tags.is_some()))
                .collect()
        };
        assert_eq!(key(&twice), key(&once));
        // The crossing node lies outside the small area: still a vertex, without its tags.
        assert!(twice
            .elements()
            .iter()
            .any(|e| e.id == 5 && e.tags.is_none()));
        assert!(twice
            .elements()
            .iter()
            .any(|e| e.id == 7 && e.tags.is_some()));
    }

    #[test]
    fn a_pbf_cut_reads_ways_and_skips_unasked_relations() {
        // osmpbf's own fixture: three nodes, one building way, one relation tagged rel_key.
        let pbf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/osmpbf-test.osm.pbf");
        let d = cut_pbf(&pbf, bbox(52.11, 11.62, 52.13, 11.64)).unwrap();
        assert_eq!(ids(&d, "node"), vec![105, 106, 108]);
        assert_eq!(ids(&d, "way"), vec![107]);
        assert!(ids(&d, "relation").is_empty());
        let w = &d.elements()[3];
        assert_eq!(w.nodes.as_deref(), Some(&[105, 106, 108, 105][..]));
        assert_eq!(w.tags.as_ref().unwrap()["building"], "yes");
        // Elsewhere, nothing.
        assert!(cut_pbf(&pbf, bbox(10.0, 10.0, 10.1, 10.1))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn geofabrik_picks_the_smallest_extract_holding_the_selection() {
        let square = |x0: f64, y0: f64, s: f64| {
            serde_json::json!({"type": "Polygon", "coordinates": [[[x0, y0], [x0 + s, y0],
                [x0 + s, y0 + s], [x0, y0 + s], [x0, y0]]]})
        };
        let feature = |url: &str, geom| serde_json::json!({"properties": {"urls": {"pbf": url}}, "geometry": geom});
        let index = serde_json::json!({"features": [
            feature("https://x/europe-latest.osm.pbf", square(0.0, 0.0, 50.0)),
            feature("https://x/romania-latest.osm.pbf", square(20.0, 40.0, 10.0)),
            feature("https://x/half-latest.osm.pbf", square(26.05, 44.4, 0.1)),
        ]});
        let want = bbox(44.445, 26.095, 44.448, 26.103);
        assert_eq!(
            geofabrik_url(&index, &want).unwrap(),
            "https://x/half-latest.osm.pbf"
        );
        // One that only holds part of it is passed over.
        let wide = bbox(44.0, 26.05, 44.2, 26.2);
        assert_eq!(
            geofabrik_url(&index, &wide).unwrap(),
            "https://x/romania-latest.osm.pbf"
        );
        assert!(geofabrik_url(&index, &bbox(70.0, 70.0, 70.1, 70.1)).is_err());
    }

    #[test]
    fn bakes_are_found_by_containment() {
        let a = bbox(1.0, 1.0, 2.0, 2.0);
        assert_eq!(E7Box::from_file_name(&a.file_name()), Some(a));
        assert!(a.contains(&bbox(1.2, 1.2, 1.8, 1.8)));
        assert!(!a.contains(&bbox(1.2, 1.2, 2.1, 1.8)));
        assert_eq!(E7Box::from_file_name("x.json.zst"), None);
    }

    #[test]
    fn pbf_and_overpass_parse_to_same_elements() {
        let pbf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/osmpbf-test.osm.pbf");
        let from_pbf = cut_pbf(&pbf, bbox(52.11, 11.62, 52.13, 11.64)).unwrap();
        // What Overpass returns for that area (`[out:json]`), element for element.
        let overpass: OsmData = serde_json::from_str(
            r#"{"elements": [
                {"type": "node", "id": 105, "lat": 52.12, "lon": 11.63},
                {"type": "node", "id": 106, "lat": 52.12, "lon": 11.63},
                {"type": "node", "id": 108, "lat": 52.12, "lon": 11.63},
                {"type": "way", "id": 107, "nodes": [105, 106, 108, 105],
                 "tags": {"building": "yes", "name": "triangle"}}]}"#,
        )
        .unwrap();
        type Key = (String, u64, Option<Vec<u64>>, Option<Vec<(String, String)>>);
        let key = |d: &OsmData| -> Vec<Key> {
            d.elements()
                .iter()
                .map(|e| {
                    let mut tags: Option<Vec<_>> =
                        e.tags.as_ref().map(|t| t.clone().into_iter().collect());
                    if let Some(t) = &mut tags {
                        t.sort();
                    }
                    (e.r#type.clone(), e.id, e.nodes.clone(), tags)
                })
                .collect()
        };
        assert_eq!(key(&from_pbf), key(&overpass));
    }

    #[test]
    fn geofabrik_picks_smallest_cover() {
        let square = |x0: f64, y0: f64, s: f64| {
            serde_json::json!({"type": "Polygon", "coordinates": [[[x0, y0], [x0 + s, y0],
                [x0 + s, y0 + s], [x0, y0 + s], [x0, y0]]]})
        };
        let feature = |url: &str, geom| serde_json::json!({"properties": {"urls": {"pbf": url}}, "geometry": geom});
        let index = serde_json::json!({"features": [
            feature("https://x/europe-latest.osm.pbf", square(-10.0, 35.0, 40.0)),
            feature("https://x/romania-latest.osm.pbf", square(20.0, 43.0, 9.0)),
            feature("https://x/hungary-latest.osm.pbf", square(16.0, 45.0, 7.0)),
        ]});
        // Bucharest sits inside Romania and Europe; Romania is the smaller.
        let bucharest = bbox(44.4450, 26.0950, 44.4480, 26.1030);
        assert_eq!(
            geofabrik_url(&index, &bucharest).unwrap(),
            "https://x/romania-latest.osm.pbf"
        );
    }
}
