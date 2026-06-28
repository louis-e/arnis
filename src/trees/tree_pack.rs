//! Schematic tree pack: bundled assets, a source abstraction, and the realm-by-location pick.

use std::borrow::Cow;

use include_dir::{include_dir, Dir};

use crate::args::Args;
use crate::trees::region::RegionLibrary;
use crate::trees::tree_library::SizeFilter;

// The bundled region tree packs (gzipped Sponge .schem grouped by realm/community).
static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/tree-packs");

/// Reads a realm pack and its vanilla-plus sprinkle from the compiled-in bundle.
pub struct TreePackSource {
    realm: String,
}

fn embedded_read(key: &str) -> Option<Cow<'static, [u8]>> {
    EMBEDDED.get_file(key).map(|f| Cow::Borrowed(f.contents()))
}

impl TreePackSource {
    pub fn embedded(realm: &str) -> Self {
        TreePackSource {
            realm: realm.to_string(),
        }
    }

    pub fn realm_manifest(&self) -> Option<Cow<'static, [u8]>> {
        embedded_read(&format!("{}/region.json", self.realm))
    }

    pub fn realm_file(&self, rel: &str) -> Option<Cow<'static, [u8]>> {
        embedded_read(&format!("{}/{rel}", self.realm))
    }

    pub fn vanilla_manifest(&self) -> Option<Cow<'static, [u8]>> {
        embedded_read("vanilla-plus/region.json")
    }

    pub fn vanilla_file(&self, rel: &str) -> Option<Cow<'static, [u8]>> {
        embedded_read(&format!("vanilla-plus/{rel}"))
    }
}

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

/// Load the region tree pack (realm from bbox center), or None for legacy procedural trees.
pub fn load(args: &Args, scale: f64, ground_level: i32) -> Option<RegionLibrary> {
    if args.legacy_trees {
        return None;
    }
    let sizes = SizeFilter::default();
    let lat = (args.bbox.min().lat() + args.bbox.max().lat()) / 2.0;
    let lon = (args.bbox.min().lng() + args.bbox.max().lng()) / 2.0;
    let realm = realm_for_latlon(lat, lon);
    let source = TreePackSource::embedded(realm);
    // No palms outside the subtropics (the wide ena realm also spans the Caribbean).
    let exclude_palms = lat.abs() > 35.0;

    match RegionLibrary::load(&source, scale, ground_level, sizes, exclude_palms) {
        Ok(lib) => {
            lib.report();
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

    #[test]
    fn realm_mapping() {
        assert_eq!(realm_for_latlon(40.71, -74.01), "ena"); // NYC: temperate, palms gated off
        assert_eq!(realm_for_latlon(25.76, -80.19), "fl"); // Miami
        assert_eq!(realm_for_latlon(34.05, -118.24), "wna"); // Los Angeles
        assert_eq!(realm_for_latlon(51.51, -0.13), "eur"); // London
        assert_eq!(realm_for_latlon(85.0, 0.0), "vanilla-plus"); // Arctic: no box matches
    }
}
