use colored::Colorize;
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

const LATEST_RELEASE_API_URL: &str = "https://api.github.com/repos/louis-e/arnis/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    // GitHub returns name/body/published_at as null (not just missing) for some releases.
    #[serde(default, deserialize_with = "null_to_empty")]
    pub name: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub body: String,
    pub html_url: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    pub published_at: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

fn null_to_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub is_newer: bool,
    pub local_version: String,
    pub remote_version: String,
    pub release: ReleaseInfo,
}

fn build_client() -> reqwest::Result<Client> {
    Client::builder()
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
}

pub fn fetch_latest_release() -> Result<ReleaseInfo, Box<dyn Error>> {
    let client = build_client()?;
    let res = client
        .get(LATEST_RELEASE_API_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()?;

    if !res.status().is_success() {
        return Err(format!(
            "GitHub API returned HTTP {}: {}",
            res.status().as_u16(),
            res.status().canonical_reason().unwrap_or("Unknown error")
        )
        .into());
    }

    Ok(res.json()?)
}

pub fn check_for_updates() -> Result<UpdateInfo, Box<dyn Error>> {
    let release = fetch_latest_release()?;
    let remote_str = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let remote_version = Version::parse(remote_str)?;
    let local_version = Version::parse(env!("CARGO_PKG_VERSION"))?;

    Ok(UpdateInfo {
        is_newer: remote_version > local_version,
        local_version: local_version.to_string(),
        remote_version: remote_version.to_string(),
        release,
    })
}

/// Fire-and-forget CLI update check; prints a one-line notice on a background thread.
pub fn check_for_updates_async() {
    std::thread::spawn(|| {
        if let Ok(info) = check_for_updates() {
            if info.is_newer {
                println!(
                    "{} {} -> {}",
                    "A new version is available:".yellow().bold(),
                    info.local_version,
                    info.remote_version
                );
            }
        }
    });
}
