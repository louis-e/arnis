//! Shared HTTP fetch + on-disk cache for Arnis-hosted archetype models.

use reqwest::blocking::ClientBuilder;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const CACHE_SUBDIR: &str = "arnis/custom_models";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const MAX_GLB_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .map(|d| d.join(CACHE_SUBDIR))
        .unwrap_or_else(|| PathBuf::from("./.arnis_custom_cache"))
}

pub(super) fn fetch_glb(url: &str, filename: &str) -> Result<Vec<u8>, String> {
    let dir = cache_root();
    let path = dir.join(filename);
    if let Ok(bytes) = fs::read(&path) {
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_GLB_BYTES {
            return Err(format!(
                "exceeds {MAX_GLB_BYTES}-byte cap (advertised {len})"
            ));
        }
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut taken = (&mut resp).take(MAX_GLB_BYTES + 1);
    taken.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_GLB_BYTES {
        return Err(format!("exceeds {MAX_GLB_BYTES}-byte cap"));
    }
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(&path, &buf);
    Ok(buf)
}
