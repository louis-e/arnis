//! Schematic tree pack: bundled assets, a source abstraction, and the realm-by-location pick.

use std::borrow::Cow;
use std::sync::Arc;

use include_dir::{include_dir, Dir};

use crate::args::Args;
use crate::coordinate_system::geographic::LLBBox;
use crate::ecoregion::{self, EcoMap};
use crate::trees::pack_dir::{PackDir, USER};
use crate::trees::region::RegionLibrary;
use crate::trees::tree_library::SizeFilter;

// The bundled region tree packs (gzipped Sponge .schem grouped by realm/community).
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/tree-packs");

/// Reads a realm pack and its vanilla-plus sprinkle from the compiled-in bundle,
/// with the trees of a user folder (`--tree-pack-dir`) added or in their place.
pub struct TreePackSource {
    realm: String,
    dir: Option<Arc<PackDir>>,
}

pub(crate) fn embedded_read(key: &str) -> Option<Cow<'static, [u8]>> {
    EMBEDDED.get_file(key).map(|f| Cow::Borrowed(f.contents()))
}

impl TreePackSource {
    #[cfg(test)]
    pub fn embedded(realm: &str) -> Self {
        Self::with_dir(realm, None)
    }

    pub fn with_dir(realm: &str, dir: Option<Arc<PackDir>>) -> Self {
        TreePackSource {
            realm: realm.to_string(),
            dir,
        }
    }

    /// The user folder, for packs loaded later from this one.
    pub fn dir(&self) -> Option<Arc<PackDir>> {
        self.dir.clone()
    }

    /// Pack directory, as ecoregion tree mixes name it.
    pub fn code(&self) -> &str {
        &self.realm
    }

    fn manifest(&self, realm: &str) -> Option<Cow<'static, [u8]>> {
        let base = embedded_read(&format!("{realm}/region.json"))?;
        match self.dir.as_ref().and_then(|d| d.manifest(realm, &base)) {
            Some(merged) => Some(Cow::Owned(merged)),
            None => Some(base),
        }
    }

    fn file(&self, realm: &str, rel: &str) -> Option<Cow<'static, [u8]>> {
        match rel.strip_prefix(USER) {
            Some(user) => self.dir.as_ref()?.read(user).map(Cow::Owned),
            None => embedded_read(&format!("{realm}/{rel}")),
        }
    }

    pub fn realm_manifest(&self) -> Option<Cow<'static, [u8]>> {
        self.manifest(&self.realm)
    }

    pub fn realm_file(&self, rel: &str) -> Option<Cow<'static, [u8]>> {
        self.file(&self.realm, rel)
    }

    pub fn vanilla_manifest(&self) -> Option<Cow<'static, [u8]>> {
        self.manifest("vanilla-plus")
    }

    pub fn vanilla_file(&self, rel: &str) -> Option<Cow<'static, [u8]>> {
        self.file("vanilla-plus", rel)
    }
}

/// Values `--tree-realm` accepts: "auto" plus every bundled realm pack.
pub const REALMS: &[&str] = &[
    "auto",
    "afr",
    "asn",
    "aus",
    "ena",
    "eur",
    "fl",
    "ind",
    "sam",
    "wna",
    "vanilla-plus",
];

/// Realm id for a point ("vanilla-plus" if none match); bounds inclusive, first match wins.
pub fn realm_for_latlon(lat: f64, lon: f64) -> &'static str {
    // (code, lat_min, lat_max, lon_min, lon_max)
    const BOXES: &[(&str, f64, f64, f64, f64)] = &[
        ("fl", 8.0, 31.0, -90.0, -60.0),
        ("ena", 8.0, 62.0, -100.0, -52.0),
        ("wna", 25.0, 72.0, -170.0, -100.0),
        ("sam", -56.0, 14.0, -82.0, -34.0),
        ("eur", 34.0, 72.0, -25.0, 40.0),
        ("afr", -36.0, 37.0, -19.0, 52.0),
        ("ind", -11.0, 29.0, 60.0, 155.0),
        ("asn", 5.0, 75.0, 40.0, 155.0),
        ("aus", -50.0, 0.0, 110.0, 180.0),
        ("aus", -50.0, 32.0, -180.0, -130.0),
    ];
    for &(code, la0, la1, lo0, lo1) in BOXES {
        if lat >= la0 && lat <= la1 && lon >= lo0 && lon <= lo1 {
            return code;
        }
    }
    "vanilla-plus"
}

/// Load the pack of the area's main ecoregion (else by bbox centre), or None for legacy trees.
pub fn load(
    args: &Args,
    bbox: LLBBox,
    scale: f64,
    ground_level: i32,
    blocks_per_meter: f64,
    ecoregions: Option<&EcoMap>,
) -> Option<RegionLibrary> {
    if args.legacy_trees {
        return None;
    }
    let mut sizes = SizeFilter::up_to(args.max_tree_size);
    if let Some(weights) = &args.tree_size_weights {
        weights.restrict(&mut sizes);
    }
    let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
    // A forced realm drops the ecoregion mixes, which would otherwise pick the communities.
    let forced = args.tree_realm.as_deref().filter(|&r| r != "auto");
    let mapped: Vec<(u16, &'static str)> = ecoregions
        .filter(|_| forced.is_none())
        .map(EcoMap::by_area)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, _)| ecoregion::tree_mix(id).map(|(pack, _)| (id, pack)))
        .collect();
    let realm = forced.unwrap_or_else(|| {
        mapped
            .first()
            .map_or_else(|| realm_for_latlon(lat, lon), |&(_, pack)| pack)
    });
    let dir = args.tree_pack_dir.as_deref().map(|root| {
        let dir = PackDir::scan(root, args.tree_pack_mode);
        dir.report();
        Arc::new(dir)
    });
    let source = TreePackSource::with_dir(realm, dir);
    let ids: Vec<u16> = mapped.iter().map(|&(id, _)| id).collect();
    // Palms stay loaded if any part of the area grows them; the ecoregion gates each cell.
    let abs_lat = lat.abs();
    let unmapped_palms = ecoregions.is_none_or(EcoMap::has_gaps) && abs_lat <= 35.0;
    let exclude_palms = forced.is_none()
        && !unmapped_palms
        && !ids
            .iter()
            .filter_map(|&id| ecoregion::lookup(id))
            .any(|eco| ecoregion::palms_belong(eco, abs_lat));

    match RegionLibrary::load(
        &source,
        scale,
        ground_level,
        blocks_per_meter,
        sizes,
        exclude_palms,
    ) {
        Ok(mut lib) => {
            if let Some(weights) = args.tree_size_weights {
                lib.set_size_weights(weights);
            }
            // Micro trees below this scale never stamp a model, so nothing to resolve.
            if forced.is_none() && scale >= crate::element_processing::tree::MICRO_TREE_MAX_SCALE {
                lib.attach_ecoregions(&ids, abs_lat);
            }
            lib.report();
            if let Some(name) = ids.first().and_then(|&id| ecoregion::name(id)) {
                match ids.len() {
                    1 => println!("  ecoregion: {name}"),
                    n => println!("  ecoregion: {name} (+{} more in the area)", n - 1),
                }
            }
            Some(lib)
        }
        Err(e) => {
            eprintln!("tree-pack: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every model a manifest lists exists and loads as a tree, and every
    /// contributed model is listed somewhere.
    #[test]
    fn every_listed_model_loads_and_none_is_orphaned() {
        let mut listed = std::collections::HashSet::new();
        for pack in EMBEDDED.dirs() {
            let code = pack.path().to_string_lossy().to_string();
            let Some(manifest) = EMBEDDED.get_file(format!("{code}/region.json")) else {
                continue;
            };
            let m: serde_json::Value = serde_json::from_slice(manifest.contents()).unwrap();
            for c in m["communities"].as_array().unwrap() {
                for sp in c["species"].as_array().unwrap() {
                    for key in ["w1", "w2", "w3"] {
                        for rel in sp[key].as_array().unwrap() {
                            let path = format!("{code}/{}", rel.as_str().unwrap());
                            if !listed.insert(path.clone()) {
                                continue;
                            }
                            let file = EMBEDDED
                                .get_file(&path)
                                .unwrap_or_else(|| panic!("{path} is listed but missing"));
                            let schem = crate::trees::schematic::load_schem(file.contents())
                                .unwrap_or_else(|e| panic!("{path}: {e}"));
                            // Cacti and snags are listed on purpose and skipped at load.
                            if path.contains("/contrib/") {
                                assert!(schem.has_leaves(), "{path} has no leaves");
                                assert!(schem.height >= 2, "{path} is flat");
                            }
                        }
                    }
                }
            }
        }
        fn walk(dir: &Dir<'static>, out: &mut Vec<String>) {
            for f in dir.files() {
                out.push(
                    f.path()
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                );
            }
            for d in dir.dirs() {
                walk(d, out);
            }
        }
        let mut files = Vec::new();
        walk(&EMBEDDED, &mut files);
        let orphans: Vec<&String> = files
            .iter()
            .filter(|f| f.contains("/contrib/") && !listed.contains(*f))
            .collect();
        assert!(orphans.is_empty(), "unlisted models: {orphans:?}");
        assert!(listed.len() > 2400, "{} models", listed.len());
    }

    #[test]
    fn every_forceable_realm_is_bundled() {
        for realm in &REALMS[1..] {
            let source = TreePackSource::embedded(realm);
            assert!(
                source.realm_manifest().is_some(),
                "{realm} has no region.json"
            );
        }
    }

    #[test]
    fn realm_mapping() {
        assert_eq!(realm_for_latlon(40.71, -74.01), "ena"); // NYC: temperate, palms gated off
        assert_eq!(realm_for_latlon(25.76, -80.19), "fl"); // Miami
        assert_eq!(realm_for_latlon(34.05, -118.24), "wna"); // Los Angeles
        assert_eq!(realm_for_latlon(51.51, -0.13), "eur"); // London
        assert_eq!(realm_for_latlon(85.0, 0.0), "vanilla-plus"); // Arctic: no box matches
    }
}
