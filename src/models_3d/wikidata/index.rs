//! Static QID → model lookup. Merges the auto-baked index with a hand-curated overlay; manual wins on conflict.

use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;

const RAW_AUTO: &str = include_str!("../../../assets/wikidata_3d_models.json");
const RAW_MANUAL: &str = include_str!("../../../assets/wikidata_3d_models_manual.json");

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
    /// Y-banded block pools; beats the OSM-derived palette when present.
    #[serde(default)]
    pub palette_layers: Option<Vec<PaletteLayer>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaletteLayer {
    pub y_max_frac: f32,
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub hex: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawIndex {
    models: HashMap<String, IndexEntry>,
}

static INDEX: Lazy<HashMap<String, IndexEntry>> = Lazy::new(|| {
    let auto: RawIndex = serde_json::from_str(RAW_AUTO).expect("wikidata_3d_models.json malformed");
    let manual: RawIndex =
        serde_json::from_str(RAW_MANUAL).expect("wikidata_3d_models_manual.json malformed");
    let mut merged = auto.models;
    merged.extend(manual.models);
    merged
});

/// Look up a Wikidata QID; returns `None` if not in the bundled index.
pub fn lookup(qid: &str) -> Option<&'static IndexEntry> {
    INDEX.get(qid)
}

/// All bundled entries sorted by label, with QID tie-breaker so duplicates render in a stable order.
pub static PERMISSIVE_ATTRIBUTIONS: Lazy<Vec<&'static IndexEntry>> = Lazy::new(|| {
    let mut v: Vec<(&String, &IndexEntry)> = INDEX.iter().collect();
    v.sort_by(|a, b| a.1.label.cmp(&b.1.label).then_with(|| a.0.cmp(b.0)));
    v.into_iter().map(|(_, e)| e).collect()
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
