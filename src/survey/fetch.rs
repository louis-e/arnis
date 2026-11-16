//! HTTP for the survey: range reads with polite retries, JSON lookups, and a
//! count of what the laser reads downloaded.

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
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Missing => write!(f, "not found"),
            Failure::Other(e) => write!(f, "{e}"),
        }
    }
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

/// A GET, paced, and retried with a growing pause while the server says it is
/// busy (429, 502, 503, 504).
fn get(
    client: &reqwest::blocking::Client,
    url: &str,
    range: Option<(u64, u64)>,
) -> Result<Vec<u8>, Failure> {
    let mut wait_ms = 1000;
    let limit = paced(url);
    for attempt in 0..8 {
        if let Some(n) = limit {
            pace(n);
        }
        let resp = {
            let _permit = crate::net::request_permit();
            let mut req = client.get(url);
            if let Some((start, len)) = range {
                req = req.header("Range", format!("bytes={}-{}", start, start + len - 1));
            }
            req.send()
                .and_then(|r| {
                    let status = r.status();
                    let retry_after = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok());
                    r.bytes().map(|b| (status, retry_after, b))
                })
                .map_err(|e| crate::net::failure_kind(&e))
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
                return Ok(body.to_vec());
            }
            Err(kind) if attempt < 2 && kind != "dns" => {
                std::thread::sleep(Duration::from_millis(wait_ms));
                wait_ms *= 2;
            }
            Err(kind) => return Err(Failure::Other(kind)),
        }
    }
    Err(Failure::Other("server busy".into()))
}

/// `len` bytes of `url` from `start`.
pub fn get_range(
    client: &reqwest::blocking::Client,
    url: &str,
    start: u64,
    len: u64,
) -> Result<Vec<u8>, Failure> {
    get(client, url, Some((start, len)))
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
