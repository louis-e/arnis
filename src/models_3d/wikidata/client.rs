//! HTTP fetcher for model files (STL or GLB) with URL-hash on-disk cache.

use crate::models_3d::{ok_status, read_capped, unless_absent, FetchError};
use fnv::FnvHasher;
use reqwest::blocking::{Client, ClientBuilder};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

const CACHE_SUBDIR: &str = "arnis/wikidata_models";
const MAX_MODEL_BYTES: u64 = 128 * 1024 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 30;

pub(crate) fn cache_root() -> PathBuf {
    if let Some(dir) = crate::elevation::cache::user_cache_dir() {
        dir.join(CACHE_SUBDIR)
    } else {
        PathBuf::from("./.arnis_wikidata_cache")
    }
}

/// Shared because model fetches fan out across the global Rayon pool, and every
/// `Client` carries its own tokio runtime and I/O driver.
fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            ClientBuilder::new()
                .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
                .user_agent(concat!(
                    "Arnis/",
                    env!("CARGO_PKG_VERSION"),
                    " (+https://github.com/louis-e/arnis)"
                ))
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn url_hash(url: &str) -> String {
    let mut h = FnvHasher::default();
    url.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Fetches a model file (STL or GLB) by URL with on-disk cache keyed by URL hash.
pub fn fetch_model(url: &str) -> Result<Vec<u8>, FetchError> {
    let dir = cache_root();
    let key = url_hash(url);
    let path = dir.join(format!("{key}.bin"));
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() >= 12 {
            return Ok(bytes);
        }
    }
    let what = format!("model {url}");
    unless_absent(&dir.join(format!("{key}.missing")), &what, || {
        let client = client()?;
        crate::net::ensure_online("3D models (Wikimedia)")?;
        let _permit = crate::net::request_permit();
        let resp = client.get(url).send().map_err(|e| e.to_string())?;
        let bytes = read_capped(ok_status(resp, &what)?, MAX_MODEL_BYTES)?;
        crate::elevation::cache::write_atomic(&path, &bytes);
        Ok(bytes)
    })
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
