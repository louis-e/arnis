//! HTTP fetcher for model files (STL or GLB) with URL-hash on-disk cache.

use fnv::FnvHasher;
use reqwest::blocking::{Client, ClientBuilder};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const CACHE_SUBDIR: &str = "arnis/wikidata_models";
const MAX_MODEL_BYTES: u64 = 128 * 1024 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub(crate) fn cache_root() -> PathBuf {
    if let Some(dir) = dirs::cache_dir() {
        dir.join(CACHE_SUBDIR)
    } else {
        PathBuf::from("./.arnis_wikidata_cache")
    }
}

fn build_client() -> Result<Client, String> {
    ClientBuilder::new()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .build()
        .map_err(|e| e.to_string())
}

fn read_capped(mut resp: reqwest::blocking::Response, cap: u64) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length() {
        if len > cap {
            return Err(format!("model exceeds {cap}-byte cap (advertised {len})"));
        }
    }
    let mut buf = Vec::new();
    let mut taken = (&mut resp).take(cap + 1);
    taken.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > cap {
        return Err(format!("model exceeds {cap}-byte cap"));
    }
    Ok(buf)
}

fn url_hash(url: &str) -> String {
    let mut h = FnvHasher::default();
    url.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Fetches a model file (STL or GLB) by URL with on-disk cache keyed by URL hash.
pub fn fetch_model(url: &str) -> Result<Vec<u8>, String> {
    let dir = cache_root();
    let path = dir.join(format!("{}.bin", url_hash(url)));
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() >= 12 {
            return Ok(bytes);
        }
    }
    let client = build_client()?;
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("model {url}: HTTP {}", resp.status()));
    }
    let bytes = read_capped(resp, MAX_MODEL_BYTES)?;
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(&path, &bytes);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_hash_is_stable() {
        let a = url_hash("https://commons.wikimedia.org/wiki/Special:FilePath/X.stl");
        let b = url_hash("https://commons.wikimedia.org/wiki/Special:FilePath/X.stl");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn url_hash_differs_for_different_urls() {
        let a = url_hash("https://commons.wikimedia.org/wiki/Special:FilePath/A.stl");
        let b = url_hash("https://commons.wikimedia.org/wiki/Special:FilePath/B.stl");
        assert_ne!(a, b);
    }
}
