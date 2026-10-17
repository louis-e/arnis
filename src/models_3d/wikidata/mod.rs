//! Wikidata `wikidata=Q*` → P4896 → Commons .stl → voxelized, placed at the OSM anchor.

pub(crate) mod client;
mod index;
mod placement;
mod stl;

pub use index::PERMISSIVE_ATTRIBUTIONS;
pub use placement::{place_wikidata_models, prescan, PrescanResult};

use crate::elevation::TallModel;
use crate::osm_parser::OsmData;
use std::collections::{HashMap, HashSet};

/// Bundled models in a raw OSM fetch with a known height, for the terrain fit.
/// Ways stand at the mean of their nodes; relations are left out.
pub fn tall_models_in(data: &OsmData) -> Vec<TallModel> {
    let mut nodes: Vec<(f64, f64, f64)> = Vec::new();
    let mut ways: Vec<(&[u64], f64)> = Vec::new();
    for e in data.elements() {
        let Some(tags) = e.tags.as_ref() else {
            continue;
        };
        let Some(entry) = tags.get("wikidata").and_then(|q| index::lookup(q.trim())) else {
            continue;
        };
        let Some(height_m) = entry
            .height_m
            .or_else(|| tags.get("height").and_then(|h| placement::parse_meters(h)))
        else {
            continue;
        };
        match (e.r#type.as_str(), e.lat, e.lon, e.nodes.as_deref()) {
            ("node", Some(lat), Some(lon), _) => nodes.push((lat, lon, height_m)),
            ("way", _, _, Some(refs)) if !refs.is_empty() => ways.push((refs, height_m)),
            _ => {}
        }
    }
    let wanted: HashSet<u64> = ways.iter().flat_map(|(r, _)| r.iter().copied()).collect();
    let coords: HashMap<u64, (f64, f64)> = if wanted.is_empty() {
        HashMap::new()
    } else {
        data.elements()
            .iter()
            .filter(|e| e.r#type == "node" && wanted.contains(&e.id))
            .filter_map(|e| Some((e.id, (e.lat?, e.lon?))))
            .collect()
    };
    let way_models = ways.into_iter().filter_map(|(refs, height_m)| {
        let pts: Vec<(f64, f64)> = refs
            .iter()
            .filter_map(|id| coords.get(id).copied())
            .collect();
        let n = pts.len() as f64;
        (n > 0.0).then(|| TallModel {
            lat: pts.iter().map(|p| p.0).sum::<f64>() / n,
            lon: pts.iter().map(|p| p.1).sum::<f64>() / n,
            height_m,
        })
    });
    nodes
        .into_iter()
        .map(|(lat, lon, height_m)| TallModel { lat, lon, height_m })
        .chain(way_models)
        .collect()
}
