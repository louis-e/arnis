//! 3DMR HTTP client + on-disk cache.

use crate::models_3d::{ok_status, read_capped, unless_absent, FetchError};
use reqwest::blocking::{Client, ClientBuilder};
use serde::{Deserialize, Deserializer};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const API_BASE: &str = "https://3dmr.eu/api";
const CACHE_SUBDIR: &str = "arnis/3dmr";
const MAX_GLB_BYTES: u64 = 64 * 1024 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 20;

#[derive(Debug, Deserialize, Clone)]
pub struct ModelInfo {
    #[allow(dead_code)]
    pub id: u64,
    pub title: Option<String>,
    pub author: Option<String>,
    #[allow(dead_code)]
    pub lat: Option<f64>,
    #[allow(dead_code)]
    pub lon: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_license")]
    pub license: Option<String>,
    /// Intrinsic yaw in degrees, clockwise from north.
    #[serde(default)]
    pub rotation: f64,
    /// Intrinsic scale: raw glTF unit × scale = real-world meters.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Intrinsic translation in meters, applied after intrinsic rotation+scale.
    #[serde(default)]
    pub translation: [f64; 3],
}

fn default_scale() -> f64 {
    1.0
}

/// Accepts `license` as either integer code or string; renders as string for attribution.
fn deserialize_license<'de, D: Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    let v = serde_json::Value::deserialize(de)?;
    Ok(match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    })
}

pub(crate) fn cache_root() -> PathBuf {
    if let Some(dir) = crate::elevation::cache::user_cache_dir() {
        dir.join(CACHE_SUBDIR)
    } else {
        PathBuf::from("./.arnis_3dmr_cache")
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

/// Fetches the latest revision metadata for a model id, with on-disk cache.
pub fn fetch_info(id: u64) -> Result<ModelInfo, FetchError> {
    let dir = cache_root();
    let info_path = dir.join(format!("{id}.json"));

    if let Ok(bytes) = fs::read(&info_path) {
        if let Ok(info) = serde_json::from_slice::<ModelInfo>(&bytes) {
            return Ok(info);
        }
    }

    unless_absent(
        &missing_marker(&dir, id),
        &format!("3DMR model {id}"),
        || {
            let client = client()?;
            let url = format!("{API_BASE}/info/{id}");
            crate::net::ensure_online("3D models (3DMR)")?;
            let _permit = crate::net::request_permit();
            let resp = client.get(&url).send().map_err(|e| e.to_string())?;
            let bytes = ok_status(resp, &format!("3DMR info {id}"))?
                .bytes()
                .map_err(|e| e.to_string())?;
            let info: ModelInfo =
                serde_json::from_slice(&bytes).map_err(|e| format!("3DMR info {id} parse: {e}"))?;

            crate::elevation::cache::write_atomic(&info_path, &bytes);

            Ok(info)
        },
    )
}

/// Fetches the latest revision binary glTF for a model id, with on-disk cache.
pub fn fetch_glb(id: u64) -> Result<Vec<u8>, FetchError> {
    let dir = cache_root();
    let glb_path = dir.join(format!("{id}.glb"));

    if let Ok(bytes) = fs::read(&glb_path) {
        if !bytes.is_empty() {
            return Ok(bytes);
        }
    }

    unless_absent(
        &missing_marker(&dir, id),
        &format!("3DMR model {id}"),
        || {
            let client = client()?;
            let url = format!("{API_BASE}/model/{id}");
            crate::net::ensure_online("3D models (3DMR)")?;
            let _permit = crate::net::request_permit();
            let resp = client.get(&url).send().map_err(|e| e.to_string())?;
            let bytes = read_capped(ok_status(resp, &format!("3DMR model {id}"))?, MAX_GLB_BYTES)?;

            crate::elevation::cache::write_atomic(&glb_path, &bytes);

            Ok(bytes)
        },
    )
}

/// One marker for both the info and the model: either missing leaves nothing to place.
fn missing_marker(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}.missing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_license_as_integer() {
        let json =
            r#"{"id": 1, "title": "x", "author": "a", "lat": null, "lon": null, "license": 0}"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.license.as_deref(), Some("0"));
    }

    #[test]
    fn parses_license_as_string() {
        let json = r#"{"id": 1, "license": "CC-BY-SA"}"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.license.as_deref(), Some("CC-BY-SA"));
    }

    #[test]
    fn parses_missing_license() {
        let json = r#"{"id": 1}"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert!(info.license.is_none());
    }
}
