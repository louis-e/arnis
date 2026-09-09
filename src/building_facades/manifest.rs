//! The preset facade set: which photographs exist, which buildings they suit,
//! and what each one measures in the real world.
//!
//! The set is compiled in, like the tree packs and the climate grid, because a
//! release is one executable with nothing beside it: a set that lived only in
//! `assets/` would never reach a user and the feature would do nothing. It
//! costs every binary about 8.8 MB, including the users who never turn it on.
//!
//! A directory still wins over the bundled set, from `--building-facades-dir`,
//! the `ARNIS_BUILDING_FACADES_DIR` variable, or an `assets/` directory beside
//! the executable, so swapping the pictures is a flag and not a rebuild.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use include_dir::{include_dir, Dir};

static EMBEDDED: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets/building-facades");

/// Where a set's files are read from.
#[derive(Clone, Debug)]
pub enum Source {
    Dir(PathBuf),
    Embedded,
}

impl Source {
    /// Cheap enough to ask once per manifest entry, which is what `validate`
    /// does rather than reading nine megabytes to find out.
    fn has(&self, file: &str) -> bool {
        match self {
            Source::Dir(dir) => dir.join(file).is_file(),
            Source::Embedded => EMBEDDED.get_file(file).is_some(),
        }
    }

    fn read(&self, file: &str) -> Option<Cow<'static, [u8]>> {
        match self {
            Source::Dir(dir) => std::fs::read(dir.join(file)).ok().map(Cow::Owned),
            Source::Embedded => EMBEDDED.get_file(file).map(|f| Cow::Borrowed(f.contents())),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Dir(dir) => write!(f, "{}", dir.display()),
            Source::Embedded => write!(f, "the bundled set"),
        }
    }
}

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
    source: Source,
    entries: Vec<Entry>,
    /// Entry indices per category name, in manifest order, so the choice does
    /// not depend on a hash map's iteration order.
    by_category: FnvHashMap<&'static str, Vec<usize>>,
    /// The decoded photographs and the scale they are at, under one lock so a
    /// picture is never handed out at a scale other than the one its caller
    /// was told.
    scaled: Mutex<Scaled>,
}

/// The photographs decoded so far, at one scale.
struct Scaled {
    /// Output pixels per real-world metre. One resample per texture, to this,
    /// is the only resampling the pixels ever see, and it is uniform in both
    /// axes by construction, so nothing is ever stretched out of proportion.
    ///
    /// Set from the requested resolution at load and lowered once, before the
    /// pending walls are cropped, to the resolution the pack will be written
    /// at (`building_facades::finalize`), so the crops come out the size the
    /// writer puts in the pack and nothing is shrunk twice.
    px_per_m: f64,
    /// `None` for an entry whose file would not decode; the chooser then skips
    /// it and the run carries on.
    images: FnvHashMap<usize, Option<std::sync::Arc<RgbImage>>>,
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

    /// Output pixels per real-world metre the photographs are at.
    pub fn px_per_m(&self) -> f64 {
        self.scaled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .px_per_m
    }

    /// Brings the set to `px_per_m` output pixels per metre. The photographs
    /// decoded at the old scale are forgotten, so the next `image` for each
    /// resamples it afresh from the file rather than from a smaller copy.
    pub fn set_px_per_m(&self, px_per_m: f64) {
        let mut cache = self.scaled.lock().unwrap_or_else(|e| e.into_inner());
        if (cache.px_per_m - px_per_m).abs() < 1e-9 {
            return;
        }
        cache.px_per_m = px_per_m;
        cache.images.clear();
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
        cache.images.clear();
        cache.images.shrink_to_fit();
    }

    /// The entry's photograph at this run's metre scale, decoded on first use,
    /// with the pixels per metre it is at, which is what a fit built on it has
    /// to be told. `None` when the file is missing or will not decode, which
    /// drops that one texture and leaves the rest of the set working.
    pub fn image(&self, index: usize) -> Option<(std::sync::Arc<RgbImage>, f64)> {
        let mut cache = self.scaled.lock().unwrap_or_else(|e| e.into_inner());
        let px_per_m = cache.px_per_m;
        if let Some(hit) = cache.images.get(&index) {
            return hit.clone().map(|img| (img, px_per_m));
        }
        let loaded = self
            .entries
            .get(index)
            .and_then(|e| load_scaled(&self.source, e, px_per_m))
            .map(std::sync::Arc::new);
        cache.images.insert(index, loaded.clone());
        loaded.map(|img| (img, px_per_m))
    }
}

/// Decodes one texture and resamples it so that one output pixel is
/// `1 / px_per_m` metres on the real building, in both axes.
///
/// This is where the manifest's metres turn into pixels, and it is the only
/// resample: everything after it crops or repeats whole pixels, so a storey
/// can never come out taller than a storey.
fn load_scaled(source: &Source, entry: &Entry, px_per_m: f64) -> Option<RgbImage> {
    let bytes = source.read(&entry.file)?;
    let img = match image::load_from_memory(&bytes) {
        // `into_rgb8` takes the decoder's own buffer when the file decoded to
        // RGB already; `to_rgb8` copies the whole image to do the same.
        Ok(img) => img.into_rgb8(),
        Err(e) => {
            eprintln!("Warning: preset facade {}: {e}", entry.file);
            return None;
        }
    };
    let (w, h) = scaled_size(entry.metres_wide, entry.metres_tall, px_per_m);
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

/// The size a photograph `metres_wide` by `metres_tall` is brought to at
/// `px_per_m`: what `load_scaled` makes, and what a fit naming a region's
/// pixels before the picture is decoded has to assume (`Fit::region_key_at`).
pub fn scaled_size(metres_wide: f64, metres_tall: f64, px_per_m: f64) -> (u32, u32) {
    (
        ((metres_wide * px_per_m).round() as u32).max(1),
        ((metres_tall * px_per_m).round() as u32).max(1),
    )
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
/// The source is checked for the file itself, so a manifest that has been
/// committed without its pictures, or a set with one image missing, drops those
/// entries at load rather than producing a bare panel for each of them later.
fn validate(raw: TextureEntry, source: &Source) -> Result<Entry, String> {
    if !safe_name(&raw.file) {
        return Err(format!("{}: not a plain file name", raw.file));
    }
    if !source.has(&raw.file) {
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
pub fn parse(
    bytes: &[u8],
    source: &Source,
    px_per_m: f64,
) -> Result<(FacadeSet, LoadReport), String> {
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
        match validate(raw, source) {
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
            source: source.clone(),
            entries,
            by_category: by_category.into_iter().collect(),
            scaled: Mutex::new(Scaled {
                px_per_m,
                images: FnvHashMap::default(),
            }),
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
    let source = Source::Dir(dir.path().to_path_buf());
    let (set, report) = parse(json.as_bytes(), &source, px_per_m).unwrap();
    (set, report, dir)
}

/// Where the facade set lives.
///
/// A directory given on the command line is the answer, right or wrong: it is
/// how a replacement set is run, and quietly falling back to the bundled set
/// when it turns out to be empty would hide the mistake behind a world full of
/// the wrong pictures, so that case is the only `None`. Without one, the
/// environment override is tried, then a directory beside the executable, and
/// the bundled set answers when neither is there.
pub fn resolve(explicit: Option<&Path>) -> Option<Source> {
    if let Some(dir) = explicit {
        return dir
            .join(MANIFEST_NAME)
            .is_file()
            .then(|| Source::Dir(dir.to_path_buf()));
    }
    let mut tried: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var(DIR_ENV) {
        if !dir.is_empty() {
            tried.push(PathBuf::from(dir));
        }
    }
    // A set dropped beside the executable, or the repository's own `assets/`
    // a few levels above a `cargo run`, so an edited picture shows without a
    // rebuild.
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent();
        for _ in 0..MAX_SEARCH_DEPTH {
            let Some(d) = dir else { break };
            tried.push(d.join("assets").join(DIR_NAME));
            dir = d.parent();
        }
    }
    Some(
        tried
            .into_iter()
            .find(|d| d.join(MANIFEST_NAME).is_file())
            .map_or(Source::Embedded, Source::Dir),
    )
}

/// Loads the set. `Err` describes what was wrong with it.
pub fn load(source: &Source, px_per_m: f64) -> Result<(FacadeSet, LoadReport), String> {
    let bytes = source
        .read(MANIFEST_NAME)
        .ok_or_else(|| format!("{source}: no {MANIFEST_NAME}"))?;
    parse(&bytes, source, px_per_m).map_err(|e| format!("{source}: {e}"))
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
        let d = Source::Dir(dir.path().to_path_buf());
        assert!(parse(br#"{"version": 1, "textures": []}"#, &d, 8.0).is_err());
        assert!(parse(b"not json", &d, 8.0).is_err());
        assert!(parse(br#"{"version": 0, "textures": []}"#, &d, 8.0).is_err());
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
        let err = match parse(raw, &Source::Dir(dir.path().to_path_buf()), 8.0) {
            Ok(_) => panic!("a set of one missing picture should not load"),
            Err(e) => e,
        };
        assert!(err.contains("no usable textures"), "{err}");

        std::fs::write(dir.path().join("gone.png"), b"").unwrap();
        let (_set, report) = parse(raw, &Source::Dir(dir.path().to_path_buf()), 8.0).unwrap();
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
        assert!(load(&Source::Dir(dir.path().to_path_buf()), 8.0).is_err());
    }

    /// The bundled set is the one a release actually ships, and nothing else
    /// in the suite reads it: a build tree always finds `assets/` on disk.
    #[test]
    fn the_bundled_set_loads_and_its_pictures_decode() {
        let (set, report) = load(&Source::Embedded, 16.0).expect("the bundled set loads");
        assert!(
            report.textures >= 100,
            "{} textures, {:?} rejected",
            report.textures,
            report.rejected
        );
        assert!(report.rejected.is_empty(), "{:?}", report.rejected);

        // One picture off each end of the manifest, decoded to the size the
        // metres and the scale ask for.
        for index in [0, set.entries().len() - 1] {
            let entry = &set.entries()[index];
            let (img, px_per_m) = set.image(index).expect("the picture decodes");
            assert!((px_per_m - 16.0).abs() < 1e-9);
            let (w, h) = scaled_size(entry.metres_wide, entry.metres_tall, 16.0);
            assert_eq!((img.width(), img.height()), (w, h), "{}", entry.file);
        }
    }

    /// Asking without a directory always finds a set: the disk copy in a build
    /// tree, the bundled one in a release. That is what makes the feature work
    /// for a user who downloads a bare executable.
    #[test]
    fn a_set_is_always_found_unless_a_bad_one_was_named() {
        assert!(resolve(None).is_some());

        // A directory named on the command line is not second guessed: falling
        // back to the bundled set would hide the mistake behind a world full of
        // the wrong pictures.
        let empty = tempfile::tempdir().unwrap();
        assert!(resolve(Some(empty.path())).is_none());
    }
}
