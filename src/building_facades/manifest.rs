//! The preset facade set: which photographs exist, which buildings they suit,
//! and what each one measures in the real world.
//!
//! # Why this is read from disk and not `include_bytes!`
//!
//! `climate.rs` embeds `koppen_0p1.bin` because every world consults the
//! climate grid, so the 6.5 MB is paid for by every user because every user
//! uses it. The facade set is the opposite case:
//!
//! * It is off by default. A user who never turns it on would still carry it.
//! * Even downsampled to the resolution the game can show, a hundred facade
//!   photographs are several megabytes, and the source set is 228 MB.
//! * The licence of the shipped set is unconfirmed (see `PROVENANCE.md`), and
//!   pixels baked into a binary cannot be swapped out by whoever redistributes
//!   it. On disk, replacing the set with a CC0 one is a manifest change and
//!   nothing else, which is the property the brief asks for.
//!
//! So the set lives in `assets/building-facades/` next to the executable, and
//! a missing or unreadable set turns the feature off with one warning rather
//! than failing a run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fnv::FnvHashMap;
use image::RgbImage;
use serde::Deserialize;

use crate::element_processing::buildings::BuildingCategory;

/// Manifest schema version this build understands. A newer file is read
/// anyway, with a note: unknown fields are ignored and the fields below have
/// not changed meaning, so refusing it would only strand users on an old
/// binary when the set they have would have worked.
pub const SCHEMA_VERSION: u32 = 1;

/// Directory name under the asset root, and the name of the manifest in it.
pub const DIR_NAME: &str = "building-facades";
pub const MANIFEST_NAME: &str = "manifest.json";

/// Environment variable pointing at a facade set, for tests and for anyone
/// running a replacement set without moving files around.
pub const DIR_ENV: &str = "ARNIS_BUILDING_FACADES_DIR";

/// How far above the executable to look for an `assets/building-facades`.
///
/// Four covers `target/release/arnis.exe` from a repository checkout and a
/// bundled layout that puts the binary a level or two inside the install.
const MAX_SEARCH_DEPTH: usize = 4;

/// One texture as the manifest describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct TextureEntry {
    /// File name inside the facade directory. A name, never a path.
    pub file: String,
    /// `BuildingCategory` variant names this texture suits.
    pub categories: Vec<String>,
    /// What the photograph spans in the real world. This is what makes a
    /// window come out window-sized on the wall.
    pub metres_wide: f64,
    pub metres_tall: f64,
    /// Storeys visible in the photograph, ground floor included.
    pub storeys: u32,
    /// Whether the left and right edges join, so the texture may repeat along
    /// a wall wider than it is.
    #[serde(default)]
    pub tiles_horizontally: bool,
    /// Whether the bottom of the image is a shopfront or entrance, which has
    /// to sit on the ground rather than be repeated up the wall.
    #[serde(default)]
    pub has_ground_floor: bool,
    /// Free text for whoever curates the set. Part of the schema so a curator
    /// can say why an entry is the way it is; deliberately never read here, so
    /// that editing a note cannot change a world.
    #[serde(default)]
    #[allow(dead_code)]
    pub note: String,
}

/// The manifest file as written.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub textures: Vec<TextureEntry>,
}

/// One entry after validation, with what the chooser and the tiler need
/// precomputed.
#[derive(Debug, Clone)]
pub struct Entry {
    pub file: String,
    pub categories: Vec<&'static str>,
    pub metres_wide: f64,
    pub metres_tall: f64,
    pub storeys: u32,
    pub tiles_horizontally: bool,
    pub has_ground_floor: bool,
    /// `metres_tall / storeys`: one floor, which is the unit the vertical
    /// repeat has to move in so windows keep lining up.
    pub storey_m: f64,
}

impl Entry {
    /// Height of the band that must stay on the ground, in metres. Zero when
    /// the texture has no ground floor of its own and so tiles from its foot.
    pub fn ground_m(&self) -> f64 {
        if self.has_ground_floor {
            self.storey_m
        } else {
            0.0
        }
    }

    /// Height of the band that repeats upwards, in metres.
    ///
    /// Counted in storeys and not as the leftover metres, because a repeat
    /// that is not a whole number of storeys puts the next copy's floor line
    /// half way up a window. Always at least one storey, so a single storey
    /// texture with a ground floor repeats itself rather than nothing.
    pub fn upper_m(&self) -> f64 {
        let upper = self
            .storeys
            .saturating_sub(u32::from(self.has_ground_floor))
            .max(1);
        f64::from(upper) * self.storey_m
    }
}

/// Every `BuildingCategory` under the name the manifest writes.
///
/// One taxonomy, not two: these are the enum's own variant names, so a
/// manifest entry either names a category the generator produces or is
/// rejected at load with the misspelling printed.
pub fn category_name(category: BuildingCategory) -> &'static str {
    match category {
        BuildingCategory::Residential => "Residential",
        BuildingCategory::House => "House",
        BuildingCategory::Farm => "Farm",
        BuildingCategory::Commercial => "Commercial",
        BuildingCategory::Office => "Office",
        BuildingCategory::Hotel => "Hotel",
        BuildingCategory::Industrial => "Industrial",
        BuildingCategory::Warehouse => "Warehouse",
        BuildingCategory::School => "School",
        BuildingCategory::Hospital => "Hospital",
        BuildingCategory::Religious => "Religious",
        BuildingCategory::TallBuilding => "TallBuilding",
        BuildingCategory::GlassySkyscraper => "GlassySkyscraper",
        BuildingCategory::GlassCornerSkyscraper => "GlassCornerSkyscraper",
        BuildingCategory::GridSkyscraper => "GridSkyscraper",
        BuildingCategory::ContemporarySkyscraper => "ContemporarySkyscraper",
        BuildingCategory::ModernSkyscraper => "ModernSkyscraper",
        BuildingCategory::MasonrySkyscraper => "MasonrySkyscraper",
        BuildingCategory::Historic => "Historic",
        BuildingCategory::Tower => "Tower",
        BuildingCategory::Garage => "Garage",
        BuildingCategory::Shed => "Shed",
        BuildingCategory::Greenhouse => "Greenhouse",
        BuildingCategory::Default => "Default",
    }
}

/// Every category name the manifest may use, in the enum's own order.
pub const CATEGORY_NAMES: [&str; 24] = [
    "Residential",
    "House",
    "Farm",
    "Commercial",
    "Office",
    "Hotel",
    "Industrial",
    "Warehouse",
    "School",
    "Hospital",
    "Religious",
    "TallBuilding",
    "GlassySkyscraper",
    "GlassCornerSkyscraper",
    "GridSkyscraper",
    "ContemporarySkyscraper",
    "ModernSkyscraper",
    "MasonrySkyscraper",
    "Historic",
    "Tower",
    "Garage",
    "Shed",
    "Greenhouse",
    "Default",
];

/// Interns a manifest category name against the enum's own names, so the rest
/// of the module compares `&'static str` pointers' contents and never a typo.
fn intern(name: &str) -> Option<&'static str> {
    CATEGORY_NAMES.iter().copied().find(|n| *n == name)
}

/// Categories a building falls back to when its own has no texture. Second
/// choices only: a house gets a small residential block before it gets a
/// warehouse, and a skyscraper gets an office block before it gets a cottage.
///
/// The last resort after this table is the `Default` category and then the
/// whole set, both handled by the chooser, so a one-entry manifest still
/// dresses every building.
pub fn related(category: BuildingCategory) -> &'static [&'static str] {
    use BuildingCategory as C;
    match category {
        C::Residential => &["House", "TallBuilding", "Hotel"],
        C::House => &["Residential", "Farm"],
        C::Farm => &["House", "Warehouse", "Shed"],
        C::Commercial => &["Office", "Residential"],
        C::Office => &["Commercial", "TallBuilding"],
        C::Hotel => &["Residential", "Office"],
        C::Industrial => &["Warehouse", "Garage"],
        C::Warehouse => &["Industrial", "Garage"],
        C::School => &["Office", "Historic"],
        C::Hospital => &["Office", "School"],
        C::Religious => &["Historic"],
        C::TallBuilding => &["Office", "Residential"],
        C::GlassySkyscraper
        | C::GlassCornerSkyscraper
        | C::GridSkyscraper
        | C::ContemporarySkyscraper
        | C::ModernSkyscraper => &["TallBuilding", "Office"],
        C::MasonrySkyscraper => &["TallBuilding", "Historic", "Office"],
        C::Historic => &["Religious", "Residential"],
        C::Tower => &["Historic", "TallBuilding"],
        C::Garage => &["Warehouse", "Shed", "Industrial"],
        C::Shed => &["Garage", "Warehouse"],
        C::Greenhouse => &["Warehouse", "Shed"],
        C::Default => &["Residential", "Commercial", "House"],
    }
}

/// A loaded facade set: the validated entries plus the directory their images
/// live in, with each image decoded and rescaled to this run's metre scale
/// only when a building first asks for it.
pub struct FacadeSet {
    dir: PathBuf,
    entries: Vec<Entry>,
    /// Entry indices per category name, in manifest order, so the choice does
    /// not depend on a hash map's iteration order.
    by_category: FnvHashMap<&'static str, Vec<usize>>,
    /// Output pixels per real-world metre. One resample per texture, to this,
    /// is the only resampling the pixels ever see, and it is uniform in both
    /// axes by construction, so nothing is ever stretched out of proportion.
    px_per_m: f64,
    /// `None` for an entry whose file would not decode; the chooser then skips
    /// it and the run carries on.
    scaled: Mutex<FnvHashMap<usize, Option<std::sync::Arc<RgbImage>>>>,
}

/// What loading a set produced, for the one-line summary and for tests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadReport {
    pub textures: usize,
    /// Entries dropped at load, with why.
    pub rejected: Vec<String>,
}

impl LoadReport {
    /// One line for the console: how many textures loaded, and what was
    /// dropped without listing a hundred missing files one at a time.
    pub fn summary(&self) -> String {
        if self.rejected.is_empty() {
            return format!("{} textures", self.textures);
        }
        format!(
            "{} textures, {} entries skipped ({}{})",
            self.textures,
            self.rejected.len(),
            self.rejected[0],
            if self.rejected.len() > 1 {
                ", and others"
            } else {
                ""
            }
        )
    }
}

impl FacadeSet {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn px_per_m(&self) -> f64 {
        self.px_per_m
    }

    /// Entry indices listing `name`, in manifest order. Empty when none do.
    pub fn in_category(&self, name: &str) -> &[usize] {
        self.by_category
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Forgets every decoded photograph.
    ///
    /// The set is decoded and rescaled once per texture and then kept, because
    /// a city asks for the same picture on hundreds of walls. Once the last
    /// candidate is placed nothing reads it again, and the world save that
    /// comes next is the run's own high-water mark, so this is tens of
    /// megabytes handed back exactly where they are worth most.
    pub fn release_images(&self) {
        let mut cache = self.scaled.lock().unwrap_or_else(|e| e.into_inner());
        cache.clear();
        cache.shrink_to_fit();
    }

    /// The entry's photograph at this run's metre scale, decoded on first use.
    /// `None` when the file is missing or will not decode, which drops that
    /// one texture and leaves the rest of the set working.
    pub fn image(&self, index: usize) -> Option<std::sync::Arc<RgbImage>> {
        let mut cache = self.scaled.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = cache.get(&index) {
            return hit.clone();
        }
        let loaded = self
            .entries
            .get(index)
            .and_then(|e| load_scaled(&self.dir, e, self.px_per_m))
            .map(std::sync::Arc::new);
        cache.insert(index, loaded.clone());
        loaded
    }
}

/// Decodes one texture and resamples it so that one output pixel is
/// `1 / px_per_m` metres on the real building, in both axes.
///
/// This is where the manifest's metres turn into pixels, and it is the only
/// resample: everything after it crops or repeats whole pixels, so a storey
/// can never come out taller than a storey.
fn load_scaled(dir: &Path, entry: &Entry, px_per_m: f64) -> Option<RgbImage> {
    let path = dir.join(&entry.file);
    let img = match image::open(&path) {
        // `into_rgb8` takes the decoder's own buffer when the file decoded to
        // RGB already; `to_rgb8` copies the whole image to do the same.
        Ok(img) => img.into_rgb8(),
        Err(e) => {
            eprintln!(
                "Warning: preset facade {} could not be read: {e}",
                path.display()
            );
            return None;
        }
    };
    let w = ((entry.metres_wide * px_per_m).round() as u32).max(1);
    let h = ((entry.metres_tall * px_per_m).round() as u32).max(1);
    if img.width() == w && img.height() == h {
        return Some(img);
    }
    Some(image::imageops::resize(
        &img,
        w,
        h,
        image::imageops::FilterType::Lanczos3,
    ))
}

/// A file name inside the facade directory, never a path: the manifest may be
/// one a user wrote, and a name with a separator, a parent segment or a drive
/// letter would read outside the folder. Same rule `facades::png_rgba` applies
/// to export file names.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        && !name.contains("..")
}

/// Validates one manifest entry. `Err` carries the reason for the summary.
///
/// `dir` is checked for the file itself, so a manifest that has been committed
/// without its pictures, or a set with one image missing, drops those entries
/// at load rather than producing a bare panel for each of them later.
fn validate(raw: TextureEntry, dir: &Path) -> Result<Entry, String> {
    if !safe_name(&raw.file) {
        return Err(format!("{}: not a plain file name", raw.file));
    }
    if !dir.join(&raw.file).is_file() {
        return Err(format!("{}: no such file in the set", raw.file));
    }
    if !(raw.metres_wide.is_finite() && raw.metres_wide > 0.0) {
        return Err(format!("{}: metres_wide must be positive", raw.file));
    }
    if !(raw.metres_tall.is_finite() && raw.metres_tall > 0.0) {
        return Err(format!("{}: metres_tall must be positive", raw.file));
    }
    if raw.storeys == 0 {
        return Err(format!("{}: storeys must be at least 1", raw.file));
    }
    let mut categories = Vec::with_capacity(raw.categories.len());
    for name in &raw.categories {
        match intern(name) {
            Some(known) => {
                if !categories.contains(&known) {
                    categories.push(known);
                }
            }
            None => return Err(format!("{}: unknown category {name}", raw.file)),
        }
    }
    if categories.is_empty() {
        return Err(format!("{}: no categories", raw.file));
    }
    let storey_m = raw.metres_tall / f64::from(raw.storeys);
    Ok(Entry {
        file: raw.file,
        categories,
        metres_wide: raw.metres_wide,
        metres_tall: raw.metres_tall,
        storeys: raw.storeys,
        tiles_horizontally: raw.tiles_horizontally,
        has_ground_floor: raw.has_ground_floor,
        storey_m,
    })
}

/// Parses a manifest's bytes and validates its entries. A single bad entry is
/// dropped with a reason; only a manifest that will not parse at all, or that
/// has no usable entry left, is an error.
pub fn parse(bytes: &[u8], dir: &Path, px_per_m: f64) -> Result<(FacadeSet, LoadReport), String> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if manifest.version == 0 {
        return Err("version must be at least 1".to_string());
    }
    if manifest.version > SCHEMA_VERSION {
        // Read anyway: every field this build knows still means what it did,
        // and refusing would strand a user whose set would have worked.
        eprintln!(
            "Warning: preset facade manifest is version {} and this build knows version \
             {SCHEMA_VERSION}; reading it and ignoring anything newer.",
            manifest.version
        );
    }
    let mut entries = Vec::new();
    let mut rejected = Vec::new();
    // BTreeMap first so the per-category lists come out in manifest order and
    // the category keys in a fixed one; the chooser must not see a hash order.
    let mut by_category: BTreeMap<&'static str, Vec<usize>> = BTreeMap::new();
    for raw in manifest.textures {
        match validate(raw, dir) {
            Ok(entry) => {
                let index = entries.len();
                for name in &entry.categories {
                    by_category.entry(name).or_default().push(index);
                }
                entries.push(entry);
            }
            Err(why) => rejected.push(why),
        }
    }
    if entries.is_empty() {
        return Err("no usable textures".to_string());
    }
    let report = LoadReport {
        textures: entries.len(),
        rejected,
    };
    Ok((
        FacadeSet {
            dir: dir.to_path_buf(),
            entries,
            by_category: by_category.into_iter().collect(),
            px_per_m,
            scaled: Mutex::new(FnvHashMap::default()),
        },
        report,
    ))
}

/// Builds a set from a manifest written inline, with an empty file on disk for
/// every picture it names, so a test can exercise the real loading path
/// (including the check that a named file is there) without shipping images.
///
/// The directory is returned with it: dropping it deletes the files, and the
/// set holds the path.
#[cfg(test)]
pub(crate) fn set_for_test(
    json: &str,
    px_per_m: f64,
) -> (FacadeSet, LoadReport, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let manifest: Manifest = serde_json::from_str(json).unwrap();
    for entry in &manifest.textures {
        if safe_name(&entry.file) {
            std::fs::write(dir.path().join(&entry.file), b"").unwrap();
        }
    }
    let (set, report) = parse(json.as_bytes(), dir.path(), px_per_m).unwrap();
    (set, report, dir)
}

/// Where the facade set lives.
///
/// A directory given on the command line is the answer, right or wrong: it is
/// how a replacement set is run, and quietly falling back to the bundled set
/// when it turns out to be empty would hide the mistake behind a world full of
/// the wrong pictures. Without one, the environment override is tried, then
/// the directory beside the executable, then the source tree, which is where a
/// `cargo run` finds them since it has no packaged assets.
pub fn resolve_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return dir.join(MANIFEST_NAME).is_file().then(|| dir.to_path_buf());
    }
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var(DIR_ENV) {
        if !dir.is_empty() {
            tried.push(PathBuf::from(dir));
        }
    }
    // Beside the executable is where a released build carries the set, and the
    // parents above it are where a build tree keeps it: `target/release/arnis`
    // is three levels below the repository's own `assets/`. Walking up finds
    // both without baking `CARGO_MANIFEST_DIR` into the binary, which is a path
    // on the build machine and resolves nowhere on a user's disk.
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent();
        for _ in 0..MAX_SEARCH_DEPTH {
            let Some(d) = dir else { break };
            tried.push(d.join("assets").join(DIR_NAME));
            dir = d.parent();
        }
    }
    tried.into_iter().find(|d| d.join(MANIFEST_NAME).is_file())
}

/// Loads the set from `dir`. `Err` describes what was wrong with it.
pub fn load(dir: &Path, px_per_m: f64) -> Result<(FacadeSet, LoadReport), String> {
    let path = dir.join(MANIFEST_NAME);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes, dir, px_per_m).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest small enough to reason about, covering both tiling kinds and
    /// both ground-floor kinds.
    pub(crate) const SAMPLE: &str = r#"{
      "version": 1,
      "textures": [
        {"file": "res_a.png", "categories": ["Residential"], "metres_wide": 12.0,
         "metres_tall": 12.0, "storeys": 4, "tiles_horizontally": true,
         "has_ground_floor": true, "note": "four storeys"},
        {"file": "res_b.png", "categories": ["Residential", "TallBuilding"],
         "metres_wide": 24.0, "metres_tall": 27.0, "storeys": 9,
         "tiles_horizontally": true, "has_ground_floor": false},
        {"file": "shop_a.png", "categories": ["Commercial"], "metres_wide": 8.0,
         "metres_tall": 6.0, "storeys": 2, "has_ground_floor": true},
        {"file": "any.png", "categories": ["Default"], "metres_wide": 10.0,
         "metres_tall": 9.0, "storeys": 3}
      ]
    }"#;

    #[test]
    fn parses_the_sample_manifest() {
        let (set, report, _dir) = set_for_test(SAMPLE, 8.0);
        assert_eq!(report.textures, 4);
        assert!(report.rejected.is_empty());
        assert_eq!(set.in_category("Residential"), &[0, 1]);
        assert_eq!(set.in_category("TallBuilding"), &[1]);
        assert_eq!(set.in_category("Nonexistent"), &[] as &[usize]);
        assert_eq!(set.entries()[0].storey_m, 3.0);
        // No ground floor: the whole image repeats, so the band is the image.
        assert_eq!(set.entries()[1].ground_m(), 0.0);
        assert_eq!(set.entries()[1].upper_m(), 27.0);
        // With one: the bottom storey stays down and the rest repeats.
        assert_eq!(set.entries()[0].ground_m(), 3.0);
        assert_eq!(set.entries()[0].upper_m(), 9.0);
    }

    #[test]
    fn every_category_name_round_trips() {
        // The manifest's vocabulary is the enum's, so a name the generator can
        // produce is always a name the manifest may write.
        for name in CATEGORY_NAMES {
            assert_eq!(intern(name), Some(name), "{name}");
        }
        assert_eq!(intern("residential"), None);
        assert_eq!(category_name(BuildingCategory::Default), "Default");
        assert_eq!(
            category_name(BuildingCategory::GridSkyscraper),
            "GridSkyscraper"
        );
    }

    #[test]
    fn related_names_are_all_real_categories() {
        for category in [
            BuildingCategory::Residential,
            BuildingCategory::House,
            BuildingCategory::Farm,
            BuildingCategory::Commercial,
            BuildingCategory::Office,
            BuildingCategory::Hotel,
            BuildingCategory::Industrial,
            BuildingCategory::Warehouse,
            BuildingCategory::School,
            BuildingCategory::Hospital,
            BuildingCategory::Religious,
            BuildingCategory::TallBuilding,
            BuildingCategory::GlassySkyscraper,
            BuildingCategory::GlassCornerSkyscraper,
            BuildingCategory::GridSkyscraper,
            BuildingCategory::ContemporarySkyscraper,
            BuildingCategory::ModernSkyscraper,
            BuildingCategory::MasonrySkyscraper,
            BuildingCategory::Historic,
            BuildingCategory::Tower,
            BuildingCategory::Garage,
            BuildingCategory::Shed,
            BuildingCategory::Greenhouse,
            BuildingCategory::Default,
        ] {
            assert!(intern(category_name(category)).is_some());
            for name in related(category) {
                assert!(intern(name).is_some(), "{name} is not a category");
            }
        }
    }

    #[test]
    fn bad_entries_are_dropped_and_the_rest_load() {
        let raw = r#"{"version": 1, "textures": [
          {"file": "../escape.png", "categories": ["House"], "metres_wide": 8.0,
           "metres_tall": 6.0, "storeys": 2},
          {"file": "zero.png", "categories": ["House"], "metres_wide": 0.0,
           "metres_tall": 6.0, "storeys": 2},
          {"file": "typo.png", "categories": ["Hous"], "metres_wide": 8.0,
           "metres_tall": 6.0, "storeys": 2},
          {"file": "nostoreys.png", "categories": ["House"], "metres_wide": 8.0,
           "metres_tall": 6.0, "storeys": 0},
          {"file": "good.png", "categories": ["House"], "metres_wide": 8.0,
           "metres_tall": 6.0, "storeys": 2}
        ]}"#;
        let (set, report, _dir) = set_for_test(raw, 8.0);
        assert_eq!(report.textures, 1);
        assert_eq!(report.rejected.len(), 4);
        assert!(report
            .rejected
            .iter()
            .any(|r| r.contains("unknown category")));
        assert_eq!(set.in_category("House"), &[0]);
    }

    #[test]
    fn an_empty_manifest_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(parse(br#"{"version": 1, "textures": []}"#, d, 8.0).is_err());
        assert!(parse(b"not json", d, 8.0).is_err());
        assert!(parse(br#"{"version": 0, "textures": []}"#, d, 8.0).is_err());
    }

    #[test]
    fn an_entry_whose_picture_is_not_there_is_dropped() {
        // The set can be shipped without its pictures, or lose one. Dropping
        // the entry at load is what keeps that from becoming a bare panel per
        // building later on.
        let dir = tempfile::tempdir().unwrap();
        let raw = br#"{"version": 1, "textures": [
          {"file": "gone.png", "categories": ["House"], "metres_wide": 8.0,
           "metres_tall": 6.0, "storeys": 2}]}"#;
        let err = match parse(raw, dir.path(), 8.0) {
            Ok(_) => panic!("a set of one missing picture should not load"),
            Err(e) => e,
        };
        assert!(err.contains("no usable textures"), "{err}");

        std::fs::write(dir.path().join("gone.png"), b"").unwrap();
        let (_set, report) = parse(raw, dir.path(), 8.0).unwrap();
        assert_eq!(report.textures, 1);
        assert_eq!(report.summary(), "1 textures");
    }

    #[test]
    fn the_load_summary_says_what_was_skipped_without_listing_everything() {
        let report = LoadReport {
            textures: 3,
            rejected: vec!["a.png: no such file in the set".to_string()],
        };
        assert_eq!(
            report.summary(),
            "3 textures, 1 entries skipped (a.png: no such file in the set)"
        );
        let report = LoadReport {
            textures: 3,
            rejected: vec!["a.png: x".to_string(), "b.png: y".to_string()],
        };
        assert!(report.summary().ends_with(", and others)"));
    }

    #[test]
    fn a_missing_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), 8.0).is_err());
    }
}
