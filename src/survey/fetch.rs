//! HTTP for the survey: range reads with polite retries, JSON lookups, and a
//! count of what the laser reads and the aerial photos downloaded.

use std::cell::Cell;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Bytes of laser data downloaded in this process, for the log.
static LASER_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn laser_bytes() -> u64 {
    LASER_BYTES.load(Ordering::Relaxed)
}

pub fn count_laser_bytes(n: u64) {
    LASER_BYTES.fetch_add(n, Ordering::Relaxed);
}

/// Bytes of aerial photos downloaded in this process: every request made on a
/// thread [`for_photos`] marked.
static PHOTO_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn photo_bytes() -> u64 {
    PHOTO_BYTES.load(Ordering::Relaxed)
}

thread_local! {
    static FOR_PHOTOS: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread as one the aerial photos are fetched on: what its
/// requests download counts as photos, toward the limit, and none starts past
/// the limit.
pub fn for_photos() {
    FOR_PHOTOS.with(|f| f.set(true));
}

fn on_photo_thread() -> bool {
    FOR_PHOTOS.with(Cell::get)
}

/// The laser data downloaded in this process past which a run stops reading:
/// laser scans and aerial photos together.
static LASER_LIMIT: AtomicU64 = AtomicU64::new(u64::MAX);

/// What a read stopped by the limit fails with.
pub const PAST_LIMIT: &str = "past the download limit";

/// Laser scans and photos downloaded in this process.
fn limited_bytes() -> u64 {
    laser_bytes() + photo_bytes()
}

/// Lets the run download `bytes` more of laser data (scans and photos) from here.
pub fn limit_laser_bytes(bytes: u64) {
    LASER_LIMIT.store(limited_bytes().saturating_add(bytes), Ordering::Relaxed);
}

/// Whether the run has downloaded more laser data (scans and photos) than it may.
pub fn past_laser_limit() -> bool {
    limited_bytes() > LASER_LIMIT.load(Ordering::Relaxed)
}

/// `Err` once past the limit, for a reader to stop on before its next request.
pub fn within_laser_limit() -> Result<(), Failure> {
    if past_laser_limit() {
        Err(Failure::Other(PAST_LIMIT.into()))
    } else {
        Ok(())
    }
}

/// Square metres of laser tiles read in this process, for the log.
static LASER_AREA: AtomicU64 = AtomicU64::new(0);

pub fn laser_area_m2() -> u64 {
    LASER_AREA.load(Ordering::Relaxed)
}

pub fn count_laser_area(m2: f64) {
    LASER_AREA.fetch_add(m2 as u64, Ordering::Relaxed);
}

/// Why a request came to nothing.
#[derive(Debug)]
pub enum Failure {
    /// The server has no such file (404, or 403 from object stores that answer
    /// so for a missing key).
    Missing,
    /// The file is there but of a kind this reader cannot use (a grid it does
    /// not know, a header out of range): skipped, as it would be every time.
    Unusable(String),
    Other(String),
}

impl Failure {
    /// The failure of a request for part of a file whose header was read: the
    /// file is there, so a part the server does not have is an error, not a
    /// missing file.
    pub fn of_part(self) -> Failure {
        match self {
            Failure::Missing => Failure::Other("part of the file is missing".into()),
            f => f,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Missing => write!(f, "not found"),
            Failure::Unusable(e) | Failure::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Whether a failure says asking for more now is no use: the host cannot be
/// reached at all (no answer, no connection, no name), so another request
/// only waits, or the run is past its download limit.
pub fn unreachable(kind: &str) -> bool {
    matches!(kind, "timeout" | "connect" | "dns" | "tls" | PAST_LIMIT)
}

/// Hosts that limit requests a second, and the most the survey starts there
/// over all its threads: IGN's Géoplateforme answers 429 past ten a second.
const PACED: &[(&str, u32)] = &[("data.geopf.fr", 8)];

/// When the next request to a paced host may start; pushed back for everyone
/// when it says it is busy.
static NEXT_START: Mutex<Option<Instant>> = Mutex::new(None);

fn paced(url: &str) -> Option<u32> {
    let host = url.split("://").nth(1)?.split('/').next()?;
    PACED.iter().find(|(h, _)| *h == host).map(|&(_, n)| n)
}

/// Wait for this request's turn.
fn pace(per_second: u32) {
    let gap = Duration::from_secs(1) / per_second;
    let wait = {
        let mut next = NEXT_START.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let start = next.map_or(now, |t| t.max(now));
        *next = Some(start + gap);
        start - now
    };
    if !wait.is_zero() {
        std::thread::sleep(wait);
    }
}

/// Hold every request back for `d`.
fn back_off(d: Duration) {
    let mut next = NEXT_START.lock().unwrap_or_else(|e| e.into_inner());
    let until = Instant::now() + d;
    if next.is_none_or(|t| t < until) {
        *next = Some(until);
    }
}

/// Most a response without a range may hold: the largest asked for whole is a
/// WMS image of 2048 pixels square or an EPT node, both far smaller.
const MAX_BODY: u64 = 256 << 20;

/// How a body read failed, as [`crate::net::failure_kind`] words a request's.
pub fn read_failure(e: &std::io::Error) -> String {
    match e.get_ref().and_then(|i| i.downcast_ref::<reqwest::Error>()) {
        Some(inner) => crate::net::failure_kind(inner),
        None if e.kind() == std::io::ErrorKind::TimedOut => "timeout".into(),
        None => "body".into(),
    }
}

/// A GET, paced, and retried with a growing pause while the server says it is
/// busy (429, 502, 503, 504). The body is read a piece at a time, so the
/// client's timeout is how long the host may go silent, not how long the
/// whole answer may take. A host that does not answer at all is not asked
/// again.
fn get(
    client: &reqwest::blocking::Client,
    url: &str,
    range: Option<Span>,
) -> Result<Vec<u8>, Failure> {
    let cap = match range {
        Some(Span::From(_, 0)) => return Ok(Vec::new()),
        Some(Span::From(_, len) | Span::Tail(len)) => len,
        None => MAX_BODY,
    };
    // A photo is not started past the limit; a laser read checks it itself.
    let photo = on_photo_thread();
    if photo {
        within_laser_limit()?;
    }
    let mut wait_ms = 1000;
    let limit = paced(url);
    for attempt in 0..8 {
        if let Some(n) = limit {
            pace(n);
        }
        let resp = {
            let _permit = crate::net::request_permit();
            let mut req = client.get(url);
            match range {
                Some(Span::From(start, len)) => {
                    let last = start.saturating_add(len - 1);
                    req = req.header("Range", format!("bytes={start}-{last}"));
                }
                Some(Span::Tail(len)) => req = req.header("Range", format!("bytes=-{len}")),
                None => {}
            }
            req.send()
                .map_err(|e| crate::net::failure_kind(&e))
                .and_then(|r| {
                    let status = r.status();
                    let retry_after = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok());
                    let mut body = Vec::new();
                    r.take(cap.saturating_add(1))
                        .read_to_end(&mut body)
                        .map_err(|e| read_failure(&e))?;
                    Ok((status, retry_after, body))
                })
        };
        match resp {
            Ok((status, retry_after, body)) => {
                let code = status.as_u16();
                if matches!(code, 429 | 502 | 503 | 504) && attempt < 7 {
                    let ms = retry_after.map_or(wait_ms, |s| (s * 1000).clamp(1000, 15_000));
                    if limit.is_some() {
                        back_off(Duration::from_millis(ms));
                    } else {
                        std::thread::sleep(Duration::from_millis(ms));
                    }
                    wait_ms = (wait_ms * 2).min(15_000);
                    continue;
                }
                if code == 404 || code == 403 {
                    return Err(Failure::Missing);
                }
                if range.is_some() && code != 206 {
                    return Err(Failure::Other(format!("HTTP {status} (expected 206)")));
                }
                if !status.is_success() {
                    return Err(Failure::Other(format!("HTTP {status}")));
                }
                if body.len() as u64 > cap {
                    return Err(Failure::Other("answer larger than asked for".into()));
                }
                if photo {
                    PHOTO_BYTES.fetch_add(body.len() as u64, Ordering::Relaxed);
                }
                return Ok(body);
            }
            Err(kind) if attempt < 2 && !matches!(kind.as_str(), "dns" | "timeout") => {
                std::thread::sleep(Duration::from_millis(wait_ms));
                wait_ms *= 2;
            }
            Err(kind) => return Err(Failure::Other(kind)),
        }
    }
    Err(Failure::Other("server busy".into()))
}

/// `len` bytes of `url` from `start`. A file stored uncompressed inside an
/// archive is named `{archive}#at={offset}`, its first byte's place in the
/// archive, and read from there.
pub fn get_range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    len: u64,
) -> Result<Vec<u8>, Failure> {
    let (url, base) = match url.rsplit_once("#at=") {
        Some((u, at)) => match at.parse::<u64>() {
            Ok(at) => (u, at),
            Err(_) => (url, 0),
        },
        None => (url, 0),
    };
    get(client, url, Some(Span::From(base + start, len)))
}

/// The last `len` bytes of `url` (an archive's directory).
pub fn get_tail(
    client: &reqwest::blocking::Client,
    url: &str,
    len: u64,
) -> Result<Vec<u8>, Failure> {
    get(client, url, Some(Span::Tail(len)))
}

/// The part of a file a request asks for.
#[derive(Clone, Copy)]
enum Span {
    /// `len` bytes from `start`.
    From(u64, u64),
    /// The last `len` bytes.
    Tail(u64),
}

/// The size of the file at `url` as the server gives it (a HEAD request): 0
/// when it has no such file, `None` when it does not say.
pub fn content_length(client: &reqwest::blocking::Client, url: &str) -> Option<u64> {
    let _permit = crate::net::request_permit();
    // Offered gzip, Bavaria's server compresses even its LAZ files on the fly
    // and then gives no length.
    let resp = client
        .head(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .timeout(Duration::from_secs(20))
        .send()
        .ok()?;
    // A file the server does not have costs nothing: the read that follows
    // finds it missing (and marks it so) without downloading anything.
    if matches!(resp.status().as_u16(), 403 | 404) {
        return Some(0);
    }
    if !resp.status().is_success() {
        return None;
    }
    resp.headers()
        .get(reqwest::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// A whole small response.
pub fn get_bytes(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, Failure> {
    get(client, url, None)
}

pub fn get_json(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<serde_json::Value, String> {
    let body = get(client, url, None).map_err(|e| format!("{url}: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("{url}: {e}"))
}
