//! Static lookup table: QID → bundled model metadata.
//!
//! Built at compile time from `assets/wikidata_3d_models.json` (refreshed by
//! `cargo run --example refresh_wikidata_index --release`). License policy is
//! permissive-only (CC0, Public Domain, CC BY any version) — the refresh
//! script rejects share-alike licenses at bake time.

use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;

const RAW: &str = include_str!("../../../assets/wikidata_3d_models.json");

#[derive(Debug, Clone, Deserialize)]
pub struct IndexEntry {
    pub label: String,
    pub url: String,
    pub license: String,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub height_m: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RawIndex {
    models: HashMap<String, IndexEntry>,
}

static INDEX: Lazy<HashMap<String, IndexEntry>> = Lazy::new(|| {
    let parsed: RawIndex = serde_json::from_str(RAW).expect("wikidata_3d_models.json malformed");
    parsed.models
});

/// Look up a Wikidata QID (e.g. "Q243"). Returns `None` if the QID is not in
/// the permissive-licensed bundled index.
pub fn lookup(qid: &str) -> Option<&'static IndexEntry> {
    INDEX.get(qid)
}

/// All entries in the bundled index, sorted by label for stable rendering in
/// the Credits modal.
pub static PERMISSIVE_ATTRIBUTIONS: Lazy<Vec<&'static IndexEntry>> = Lazy::new(|| {
    let mut v: Vec<&IndexEntry> = INDEX.values().collect();
    v.sort_by(|a, b| a.label.cmp(&b.label));
    v
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_loads_and_is_nonempty() {
        assert!(!INDEX.is_empty(), "bundled index unexpectedly empty");
    }

    #[test]
    fn all_entries_have_required_fields() {
        for (qid, e) in INDEX.iter() {
            assert!(!e.label.is_empty(), "empty label for {qid}");
            assert!(e.url.starts_with("https://"), "non-https url for {qid}");
            assert!(!e.license.is_empty(), "empty license for {qid}");
        }
    }
}
