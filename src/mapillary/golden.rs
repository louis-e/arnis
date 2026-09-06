//! Reading the golden fixtures under `tests/golden/facade/`.
//!
//! Test-only plumbing, so each stage's golden test lives in that stage's own
//! file and only the loading lives here. The fixtures come from the Python lab
//! run over `cache_munich_test` with the reference run `out/munich_plane`; the
//! `manifest.json` of each folder says what every file is, what the counts are,
//! what run it came from and what tolerance each stage is held to.
//!
//! Numbers that must hold on this box: 173 buildings of which 69 are targets,
//! 1030 walls, 1076 cameras of which 288 are panoramas, and 103 tier A walls.
//!
//! # The fixtures are optional
//!
//! They are an export of one Python run, not something a clone can rebuild:
//! reproducing them needs the lab, a 5.8 GB Mapillary image cache and a 928 MB
//! run directory, none of which is in this repository. So every test that reads
//! them opens with `if golden::absent() { return; }`, and a tree without the
//! folder runs everything else and goes green.
//!
//! The export has two halves that cost very differently, so they can be carried
//! separately. The numbers are 278 files, 14 MB on disk and 5 MB packed, and
//! sixteen of the twenty-six comparisons run on them alone. The images are 390
//! files and 27 MB that compress to nothing, and hold the other ten. Measured on
//! 2026-09-06, the whole suite takes 15 s with no fixtures, 90 s with the
//! numbers and 445 s with both, so the images are 83 per cent of the download
//! and 80 per cent of the wall clock. A test that opens an image therefore opens
//! with `pixels_absent` rather than `absent`, so an export carrying only its
//! numbers still runs the sixteen.
//!
//! # Regenerating them
//!
//! The lab is its own repository, <https://github.com/louis-e/orthofacade>,
//! checked out at `tools/facade_lab/` (which this repository ignores). From
//! there, with a Mapillary token in `MAPILLARY_TOKEN`:
//!
//! ```text
//! set PYTHONIOENCODING=utf-8
//! python prefetch.py --bbox 48.135635,11.578243,48.137225,11.580818 ^
//!     --cache-dir cache_munich_test --all-panos
//! python run.py --run munich_plane --cache-dir cache_munich_test ^
//!     --stage geometry,align,texture --workers 16
//! python export_golden.py            --run munich_plane --cache-dir cache_munich_test
//! python export_golden_sfm.py        --run munich_plane --cache-dir cache_munich_test
//! python export_golden_confidence.py --run munich_plane
//! python export_golden_refine.py     --run munich_plane
//! python export_golden_texture.py    --run munich_plane
//! python export_golden_openings.py   --run munich_plane
//! ```
//!
//! The first two steps are the expensive ones: the prefetch downloads 5.8 GB of
//! imagery and SfM clusters, and the run writes 928 MB. The six exporters then
//! read that run and write only `tests/golden/facade/`. The run is
//! deterministic, so a re-export of an unchanged lab reproduces the fixtures
//! byte for byte; `PORT_REQUIREMENTS.md` records the check.
//!
//! **Delete `tests/golden/facade/` before re-exporting.** The exporters write
//! over what they produce and never remove anything, and each of them samples
//! its own walls and views, so a sample that changes between runs leaves the
//! previous run's per wall files behind. That is how 219 files and 9 MB of dead
//! fixtures accumulated by 2026-09-06 without any manifest naming them.

#![cfg(test)]
// The fixture structs mirror the files field for field, including the fields the
// stages that are not ported yet will want. Deserialising a field nobody reads
// is what keeps the shape honest, so unused ones are expected here.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use super::types::{BBox, Frame};

/// The fixture directory.
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("facade")
}

/// Whether the fixture export is in this tree.
///
/// The top level manifest and not the folder, because a `git clean` or a
/// half finished copy leaves the directory behind, and half an export fails in
/// a way that reads like a broken port rather than a missing download.
pub fn present() -> bool {
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| dir().join("manifest.json").is_file())
}

/// True when the fixtures are absent, having said so. The first line of every
/// golden test, so that a tree without the export skips the comparison instead
/// of failing it:
///
/// ```text
/// if golden::absent() {
///     return;
/// }
/// ```
#[must_use]
pub fn absent() -> bool {
    if present() {
        return false;
    }
    static BANNER: std::sync::Once = std::sync::Once::new();
    BANNER.call_once(|| {
        banner(
            "is not in this tree, so every golden test that compares the facade \
             pipeline against the Python reference run is skipped",
        );
    });
    skipped("the golden fixtures are not in this tree");
    true
}

/// Whether the export's images are in this tree as well as its numbers.
///
/// The two halves are worth separating because they cost so differently: the
/// 390 image files are 27 MB of the export's 41 and, being compressed already,
/// are 83 per cent of what a clone would pay for the whole thing and 80 per
/// cent of what the golden tests cost to run, while the numbers are 5 MB packed
/// and pin sixteen of the twenty-six stage comparisons on their own. So the
/// images can be left out as a class, and one probe answers for all of them:
/// leaving out some but not others is not a shape the exporters produce.
pub fn pixels_present() -> bool {
    static PIXELS: OnceLock<bool> = OnceLock::new();
    *PIXELS.get_or_init(|| present() && holds_an_image(&dir()))
}

fn holds_an_image(d: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(d) else {
        return false;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if holds_an_image(&p) {
                return true;
            }
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("png" | "jpg" | "jpeg")
        ) {
            return true;
        }
    }
    false
}

/// True when the fixtures this test reads pixels from are absent, having said
/// so. The first line of a golden test that opens an image, in place of
/// `absent`, so that an export carrying only its numbers still runs the
/// sixteen comparisons that need nothing else.
#[must_use]
pub fn pixels_absent() -> bool {
    if absent() {
        return true;
    }
    if pixels_present() {
        return false;
    }
    static BANNER: std::sync::Once = std::sync::Once::new();
    BANNER.call_once(|| banner("carries its numbers but not its images, so the golden tests that resample, fuse or classify pixels are skipped"));
    skipped("the golden fixtures carry no images");
    true
}

/// One line on the real stderr, once per test binary. libtest captures
/// `println!` from a test that passes, so a skip announced that way is
/// invisible in a plain `cargo test` and a run that quietly drops the
/// comparison against the Python looks like a run that made it. The raw handle
/// is not captured.
fn banner(what: &str) {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr(),
        "\nnote: {} {}. The fixtures are an optional export of the Python \
         reference run and are not part of a clone; src/mapillary/golden.rs \
         says what they are and how to regenerate them. Everything else still \
         runs.\n",
        dir().display(),
        what
    );
}

/// The per test line, which `--nocapture` shows. libtest names each test thread
/// after its test, which is the only handle a test has on its own name;
/// `--test-threads=1` runs them on the main thread instead.
fn skipped(why: &str) {
    println!(
        "skipped, {why}: {}",
        std::thread::current().name().unwrap_or("this test")
    );
}

/// One fixture file as raw JSON.
pub fn load_value(name: &str) -> Value {
    let path = dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read golden fixture {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("cannot parse golden fixture {}: {e}", path.display()))
}

/// One fixture file, typed.
pub fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_value(load_value(name))
        .unwrap_or_else(|e| panic!("golden fixture {name} does not fit its type: {e}"))
}

#[derive(Debug, Deserialize)]
pub struct GoldenFrame {
    pub lon0: f64,
    pub lat0: f64,
    pub radius_m: f64,
    /// `(min_lat, min_lon, max_lat, max_lon)`, the order prefetch records.
    pub bbox: [f64; 4],
}

impl GoldenFrame {
    pub fn bbox(&self) -> BBox {
        BBox::new(self.bbox[0], self.bbox[1], self.bbox[2], self.bbox[3])
    }

    pub fn frame(&self) -> Frame {
        Frame::new(self.lon0, self.lat0)
    }
}

#[derive(Debug, Deserialize)]
pub struct GoldenEdge {
    pub edge_idx: usize,
    pub node_a: i64,
    pub node_b: i64,
    pub s0: f64,
    pub s1: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenWall {
    pub key: String,
    pub building_key: String,
    pub idx: usize,
    pub node_a: i64,
    pub node_b: i64,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub n: [f64; 2],
    pub length: f64,
    pub merged_idx: Vec<usize>,
    pub piece: usize,
    pub n_pieces: usize,
    pub height_osm: Option<f64>,
    pub height_source: String,
    pub reachable: bool,
    pub s_offset: f64,
    pub edges: Vec<GoldenEdge>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenBuilding {
    pub key: String,
    pub osm_id: i64,
    pub kind: String,
    pub ring: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
    pub node_ids: Vec<i64>,
    pub height_osm: Option<f64>,
    pub height_source: String,
    pub min_height: f64,
    pub target: bool,
}

/// A camera as `pose::camera_from_meta` builds it, before `sfm.rs` moves it.
#[derive(Debug, Deserialize)]
pub struct GoldenCamera {
    pub id: String,
    pub camera_type: String,
    pub camera_params: Option<Vec<f64>>,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub pose_source: String,
    pub pose_factor: f64,
    pub roll_deg: f64,
    pub pitch_deg: f64,
    pub compass_deg: f64,
    pub heading_deg: f64,
    pub cam_height_m: f64,
    pub ground_z: f64,
    pub cluster_id: Option<String>,
    pub sequence: String,
    pub captured_at: i64,
    pub quality: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenCameraModel {
    pub camera_type: String,
    pub camera_params: Vec<f64>,
    pub images: usize,
    pub radial_limit: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenPoint {
    #[serde(rename = "P")]
    pub p: [f64; 3],
    /// `None` when the point has no projection at all.
    pub u: Option<f64>,
    pub v: Option<f64>,
    pub length_m: f64,
    pub px: Option<f64>,
    pub py: Option<f64>,
    pub inside: bool,
}

#[derive(Debug, Deserialize)]
pub struct GoldenInverse {
    pub u: f64,
    pub v: f64,
    pub dir: [f64; 3],
}

#[derive(Debug, Deserialize)]
pub struct GoldenProjectionCase {
    pub camera_id: String,
    pub camera_type: String,
    pub camera_params: Option<Vec<f64>>,
    pub width: u32,
    pub height: u32,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub radial_limit: Option<f64>,
    pub points: Vec<GoldenPoint>,
    #[serde(default)]
    pub inverse: Vec<GoldenInverse>,
}

/// The trimmed Graph API records, parsed once per test binary: the file is
/// 660 KB and several tests want it.
pub fn panos() -> &'static Vec<Value> {
    static PANOS: OnceLock<Vec<Value>> = OnceLock::new();
    PANOS.get_or_init(|| match load_value("panos.json") {
        Value::Array(a) => a,
        other => panic!("panos.json is not an array but {other:?}"),
    })
}

/// Largest absolute difference between two arrays of the same shape.
pub fn max_abs_diff(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f64, f64::max)
}

/// Angle in degrees between two plan vectors.
pub fn angle_between_deg(a: [f64; 2], b: [f64; 2]) -> f64 {
    let na = (a[0] * a[0] + a[1] * a[1]).sqrt();
    let nb = (b[0] * b[0] + b[1] * b[1]).sqrt();
    if na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    let cos = ((a[0] * b[0] + a[1] * b[1]) / (na * nb)).clamp(-1.0, 1.0);
    cos.acos().to_degrees()
}

// --------------------------------------------------------------------------- the block product

/// The per wall fixtures of the opening and band stages, under
/// `tests/golden/facade/openings/`. One `<key>_tex.png` is the only image
/// input; the JSON carries the rest of the input and every product, including
/// the rectangles, because when a grid disagrees the rectangle list is what
/// says which rule went a different way.
///
/// Regenerate with:
///
/// ```text
/// set PYTHONIOENCODING=utf-8
/// python tools/facade_lab/export_golden_openings.py --run munich_plane
/// ```
pub fn openings_dir() -> PathBuf {
    dir().join("openings")
}

#[derive(Debug, Deserialize)]
pub struct GoldenOpeningsManifest {
    pub run: String,
    pub walls: Vec<GoldenOpeningsEntry>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenOpeningsEntry {
    pub key: String,
    pub tier: String,
    pub cols: usize,
    pub rows: usize,
    pub windows: usize,
    pub doors: usize,
    pub bands: usize,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRect {
    pub kind: String,
    pub reason: String,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub contrast: f64,
    pub chroma: f64,
    #[serde(rename = "std_L")]
    pub std_l: f64,
    pub fill: f64,
    pub n_cols: usize,
    pub n_rows: usize,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRhythm {
    pub pitch: f64,
    pub phase: f64,
    pub support: f64,
    pub n_windows: usize,
    pub accepted: bool,
}

#[derive(Debug, Deserialize)]
pub struct GoldenOpeningsProduct {
    pub cls: Vec<u8>,
    pub rgb: Vec<u8>,
    pub evidence: Vec<f64>,
    pub rects: Vec<GoldenRect>,
    pub floors: Vec<[f64; 2]>,
    pub rhythm: GoldenRhythm,
}

#[derive(Debug, Deserialize)]
pub struct GoldenBand {
    pub r0: usize,
    pub r1: usize,
    pub rgb: Option<[u8; 3]>,
    pub lab: Option<[f64; 3]>,
    pub window_rgb: Option<[u8; 3]>,
    /// The block the lab's own picker lands this band colour on.
    pub block: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenLattice {
    pub period_m: Option<usize>,
    pub width: usize,
    pub phase: usize,
    pub score: f64,
    pub on_mean: f64,
    pub cover: f64,
    pub accepted: bool,
    pub cols: Vec<usize>,
    pub window_cols: Vec<usize>,
    pub rows: Vec<usize>,
    pub row_period_m: Option<usize>,
    pub row_score: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenStructure {
    pub cls: Vec<u8>,
    pub rgb: Vec<u8>,
    pub added: Vec<u8>,
    pub bands: Vec<GoldenBand>,
    pub lattice: GoldenLattice,
    pub door_rgb: [u8; 3],
    pub sky_rows: usize,
    pub sky_cells: usize,
}

#[derive(Debug, Deserialize)]
pub struct GoldenOpeningsWall {
    pub key: String,
    pub tier: String,
    pub cols: usize,
    pub rows: usize,
    pub origin_px: [i32; 2],
    pub observed: Option<Vec<u8>>,
    pub openings: GoldenOpeningsProduct,
    pub structure: GoldenStructure,
}

impl GoldenOpeningsWall {
    /// The wall's 8 px per metre texture: RGB plus the alpha channel as the
    /// validity mask, which is how the export writes it.
    pub fn texture(&self) -> super::openings::WallTexture {
        let path = openings_dir().join(format!("{}_tex.png", self.key));
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        assert_eq!(
            (w, h),
            (self.cols * 8, self.rows * 8),
            "{}: texture does not match the grid",
            self.key
        );
        let rgb = img.pixels().map(|p| [p.0[0], p.0[1], p.0[2]]).collect();
        let valid = img.pixels().map(|p| p.0[3] > 0).collect();
        super::openings::WallTexture::new(w, h, rgb, valid)
    }

    pub fn observed_mask(&self) -> Option<Vec<bool>> {
        self.observed
            .as_ref()
            .map(|o| o.iter().map(|v| *v != 0).collect())
    }
}

/// Every wall of the opening fixture, in manifest order.
pub fn openings_walls() -> Vec<GoldenOpeningsWall> {
    let manifest: GoldenOpeningsManifest = load("openings/manifest.json");
    manifest
        .walls
        .iter()
        .map(|e| load(&format!("openings/{}.json", e.key)))
        .collect()
}

// --------------------------------------------------------------------------- confidence

/// Per wall confidence fixtures, `tests/golden/facade/confidence.json`.
///
/// Regenerate with:
///
/// ```text
/// set PYTHONIOENCODING=utf-8
/// python tools/facade_lab/export_golden_confidence.py --run munich_plane
/// ```
#[derive(Debug, Deserialize)]
pub struct GoldenConfidenceFile {
    pub run: String,
    pub count: usize,
    pub walls: Vec<GoldenConfidenceWall>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenConfidenceWall {
    pub key: String,
    pub factors: GoldenFactors,
    pub confidence: f64,
    pub tier: String,
    pub unknown_share: f64,
    pub agreement_m: f64,
    pub n_views: usize,
    pub height_source: String,
    pub osm_source: Option<String>,
    pub flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenFactors {
    pub pose: f64,
    pub registration: f64,
    pub plane: f64,
    pub extent: f64,
    pub height: f64,
    pub lean: f64,
    pub plane_gate: f64,
    pub views: f64,
    pub occlusion: f64,
    pub image: f64,
}

pub fn confidence_walls() -> GoldenConfidenceFile {
    load("confidence.json")
}

// --------------------------------------------------------------------------- sfm and plane

/// The cluster, ground, plane and depth fixtures under
/// `tests/golden/facade/sfm/`.
///
/// `export_golden.py` stops at the pose stage because the cluster clouds are
/// 675 MB on disk; this folder holds the part of them the sfm and plane stages
/// can actually reach. **The point subset is exact, not a sample**: every stage
/// filters the cloud before it uses it, and only the points that survive a
/// filter are shipped, so the answers over the subset are the answers over the
/// whole cloud. The exporter's own docstring lists the three filters.
///
/// Regenerate with:
///
/// ```text
/// set PYTHONIOENCODING=utf-8
/// python tools/facade_lab/export_golden_sfm.py --run munich_plane
/// ```
pub fn sfm_dir() -> PathBuf {
    dir().join("sfm")
}

/// One fixture file under `sfm/`, typed.
pub fn load_sfm<T: serde::de::DeserializeOwned>(name: &str) -> T {
    load(&format!("sfm/{name}"))
}

#[derive(Debug, Deserialize)]
pub struct GoldenShot {
    pub id: String,
    pub rotation: [f64; 3],
    pub translation: [f64; 3],
    pub capture_time: f64,
    pub skey: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenCluster {
    pub id: String,
    /// `(lon, lat, alt)` of the reconstruction's own origin.
    pub reference_lla: [f64; 3],
    pub points_file: String,
    /// Points in the fixture, against `points_full` in the whole cluster.
    pub points: usize,
    pub points_full: usize,
    pub metric_ratio: Option<f64>,
    pub scale_ok: bool,
    pub mapped_shots: usize,
    /// pano id to shot id, as the run mapped them.
    pub shot_map: std::collections::BTreeMap<String, String>,
    pub shots: Vec<GoldenShot>,
}

impl GoldenCluster {
    /// The cluster as `fetch::RawCluster::parse` would have handed it over,
    /// with the point subset in its own topocentric frame.
    pub fn raw(&self) -> super::fetch::RawCluster {
        let path = sfm_dir().join(&self.points_file);
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        assert_eq!(
            bytes.len(),
            self.points * 24,
            "{} is not {} float64 triples",
            self.points_file,
            self.points
        );
        let points: Vec<[f64; 3]> = bytes
            .chunks_exact(24)
            .map(|c| {
                let f =
                    |k: usize| f64::from_le_bytes(c[k * 8..k * 8 + 8].try_into().expect("8 bytes"));
                [f(0), f(1), f(2)]
            })
            .collect();
        super::fetch::RawCluster {
            cluster_id: self.id.clone(),
            ref_lla: (
                self.reference_lla[0],
                self.reference_lla[1],
                self.reference_lla[2],
            ),
            shots: self
                .shots
                .iter()
                .map(|s| super::fetch::RawShot {
                    shot_id: s.id.clone(),
                    rotation: s.rotation,
                    translation: s.translation,
                    capture_time: s.capture_time,
                    skey: s.skey.clone(),
                    compass: f64::NAN,
                    camera: String::new(),
                })
                .collect(),
            colors: vec![[0, 0, 0]; points.len()],
            points,
        }
    }
}

/// What `sfm.ground_level` counted under one camera, and what the ladder made
/// of it. Not `GoldenGround`, which is the refine stage's ground row.
#[derive(Debug, Deserialize)]
pub struct GoldenGroundLevel {
    pub id: String,
    pub n_points: usize,
    pub ground_z: f64,
    pub cam_height_m: f64,
    pub ground_source: String,
    pub cluster_id: Option<String>,
    pub shot_id: Option<String>,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
}

/// A registered camera, as the plane and depth fixtures carry it.
#[derive(Debug, Deserialize)]
pub struct GoldenRegCamera {
    pub pano_id: String,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub camera_type: String,
    pub camera_params: Option<Vec<f64>>,
    pub width: u32,
    pub height: u32,
    pub ground_z: f64,
    pub cam_height_m: f64,
    pub cluster_id: String,
    pub reg_accepted: bool,
    /// `(dx, dy, theta_deg)`, zero when the registration was not accepted.
    pub shift: [f64; 3],
}

impl GoldenRegCamera {
    pub fn camera(&self) -> super::types::Camera {
        use super::types::{Camera, CameraModel, GroundSource, PoseSource, RegResult};
        Camera {
            pano_id: self.pano_id.clone(),
            centre: self.centre,
            axes: self.axes,
            pose_source: PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: self.ground_z,
            cam_height_m: self.cam_height_m,
            ground_source: GroundSource::Cloud,
            cluster_id: Some(self.cluster_id.clone()),
            shot_id: None,
            reg: Some(RegResult {
                dx: self.shift[0],
                dy: self.shift[1],
                theta_deg: self.shift[2],
                accepted: self.reg_accepted,
                ..RegResult::default()
            }),
            compass_deg: 0.0,
            pose_factor: 1.0,
            width: self.width,
            height: self.height,
            camera_type: CameraModel::parse(&self.camera_type),
            camera_params: self.camera_params.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GoldenFit {
    pub source: String,
    pub n: [f64; 2],
    pub d: f64,
    pub n_inliers: usize,
    pub pts_per_m: f64,
    pub rms_m: f64,
    pub angle_vs_osm_deg: f64,
    pub offset_vs_osm_m: f64,
    pub z_top98: Option<f64>,
    pub z_continuous: bool,
    pub ambiguity: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenFrameFit {
    pub pano_id: Option<String>,
    pub source: String,
    pub n: [f64; 2],
    pub d: f64,
    pub a_ref: Option<[f64; 2]>,
    pub b_ref: Option<[f64; 2]>,
    pub src_a: Option<String>,
    pub src_b: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenViewOffset {
    pub dn_m: f64,
    pub ds_m: f64,
    pub dtheta_deg: f64,
}

/// One (wall, view) plane fit of the reference run with everything it was fed.
#[derive(Debug, Deserialize)]
pub struct GoldenPlaneSample {
    pub wall_key: String,
    pub camera: GoldenRegCamera,
    pub fit: GoldenFit,
    pub z_base: f64,
    pub z_base_source: String,
    pub h_cloud: Option<f64>,
    pub frame_fit: GoldenFrameFit,
    pub offset_in_frame: GoldenViewOffset,
}

/// One camera's depth map, with the inputs that made it.
#[derive(Debug, Deserialize)]
pub struct GoldenDepth {
    pub camera: GoldenRegCamera,
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub radius_m: f64,
    pub points_in_radius: usize,
    pub finite_cells: usize,
    pub min_depth_m: Option<f64>,
}

impl GoldenDepth {
    /// The stored map: a 16-bit PNG of the float16 bit pattern of each cell,
    /// decoded back to `f32` with `0x7C00` as the empty cell.
    pub fn depth(&self) -> Vec<f32> {
        let path = sfm_dir().join(&self.file);
        let img = image::open(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .to_luma16();
        assert_eq!(
            (img.width(), img.height()),
            (self.width, self.height),
            "{} is the wrong size",
            self.file
        );
        img.pixels().map(|p| f16_bits_to_f32(p.0[0])).collect()
    }
}

/// One float16 bit pattern as the `f32` of the same value.
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = f32::from(bits & 0x03ff);
    let magnitude = match exponent {
        0 => mantissa * 2.0f32.powi(-24),
        0x1f if bits & 0x03ff == 0 => f32::INFINITY,
        0x1f => f32::NAN,
        e => (1.0 + mantissa / 1024.0) * 2.0f32.powi(i32::from(e) - 15),
    };
    if bits & 0x8000 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

impl GoldenWall {
    /// The fixture wall as the pipeline's own type.
    pub fn wall(&self) -> super::types::Wall {
        use super::types::{HeightSource, Wall, WallEdge};
        Wall {
            key: self.key.clone(),
            building_key: self.building_key.clone(),
            idx: self.idx,
            node_a: self.node_a,
            node_b: self.node_b,
            a: self.a,
            b: self.b,
            n: self.n,
            length: self.length,
            merged_idx: self.merged_idx.clone(),
            piece: self.piece,
            n_pieces: self.n_pieces,
            height_osm: self.height_osm,
            height_source: HeightSource::from_str_or_default(&self.height_source),
            reachable: self.reachable,
            unreachable_reason: String::new(),
            edges: self
                .edges
                .iter()
                .map(|e| WallEdge {
                    edge_idx: e.edge_idx,
                    node_a: e.node_a,
                    node_b: e.node_b,
                    s0: e.s0,
                    s1: e.s1,
                })
                .collect(),
            s_offset: self.s_offset,
        }
    }
}

/// A camera after `sfm.py` has moved it: `cameras_geometry.json`.
///
/// The camera model is not repeated here; join on id with `cameras_pose.json`.
/// The difference between the two files is exactly what the sfm stage owes.
#[derive(Debug, Deserialize)]
pub struct GoldenGeometryCamera {
    pub id: String,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub pose_source: String,
    pub roll_deg: f64,
    pub pitch_deg: f64,
    pub compass_deg: f64,
    pub ground_z: f64,
    pub cam_height_m: f64,
    pub ground_source: String,
    pub cluster_id: Option<String>,
    pub shot_id: Option<String>,
    pub captured_at: i64,
    pub quality: Option<f64>,
}

// --------------------------------------------------------------------------- visibility

/// `candidates.json`: every (wall, camera) pair the geometry stage looked at on
/// the walls it kept, with the value and the verdict of every gate.
///
/// `gate_pass` and `gate_value` are parallel to `gate_names`, with `None` where
/// a gate does not apply to that camera class.
#[derive(Debug, Deserialize)]
pub struct GoldenCandidateFile {
    pub gate_names: Vec<String>,
    pub candidates: Vec<GoldenCandidate>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenCandidate {
    pub wall_key: String,
    pub pano_id: String,
    pub dist_m: f64,
    pub incidence_deg: f64,
    pub angwidth_deg: f64,
    pub g_score: f64,
    pub gate_pass: Vec<Option<u8>>,
    pub gate_value: Vec<Option<f64>>,
    pub rejected_reason: Option<String>,
}

impl GoldenCandidateFile {
    /// The value and verdict of one gate, or `None` where it does not apply.
    pub fn gate(&self, cand: &GoldenCandidate, name: &str) -> Option<(bool, f64)> {
        let i = self.gate_names.iter().position(|n| n == name)?;
        match (cand.gate_pass[i], cand.gate_value[i]) {
            (Some(p), Some(v)) => Some((p != 0, v)),
            _ => None,
        }
    }
}

/// The cameras the geometry stage handed to the visibility gates: the pose
/// stage's camera model joined with the position and ground the sfm stage
/// replaced.
pub fn geometry_cameras() -> std::collections::BTreeMap<String, super::types::Camera> {
    use super::types::{Camera, CameraModel, GroundSource, PoseSource, RegResult};
    let pose: Vec<GoldenCamera> = load("cameras_pose.json");
    let pose: std::collections::HashMap<&str, &GoldenCamera> =
        pose.iter().map(|c| (c.id.as_str(), c)).collect();
    let geom: Vec<GoldenGeometryCamera> = load("cameras_geometry.json");
    let mut out = std::collections::BTreeMap::new();
    for g in &geom {
        let p = pose[g.id.as_str()];
        out.insert(
            g.id.clone(),
            Camera {
                pano_id: g.id.clone(),
                centre: g.centre,
                axes: g.axes,
                pose_source: PoseSource::from_str_or_default(&g.pose_source),
                roll_deg: g.roll_deg,
                pitch_deg: g.pitch_deg,
                ground_z: g.ground_z,
                cam_height_m: g.cam_height_m,
                ground_source: GroundSource::from_str_or_default(&g.ground_source),
                cluster_id: g.cluster_id.clone(),
                shot_id: g.shot_id.clone(),
                reg: None::<RegResult>,
                compass_deg: g.compass_deg,
                pose_factor: PoseSource::from_str_or_default(&g.pose_source).factor(),
                width: p.width,
                height: p.height,
                camera_type: CameraModel::parse(&p.camera_type),
                camera_params: p.camera_params.clone().unwrap_or_default(),
            },
        );
    }
    out
}

/// Every wall of the box, keyed.
pub fn walls_by_key() -> std::collections::BTreeMap<String, super::types::Wall> {
    let walls: Vec<GoldenWall> = load("walls.json");
    walls.iter().map(|w| (w.key.clone(), w.wall())).collect()
}

/// Every footprint of the box as the pipeline's own type.
pub fn buildings() -> Vec<super::types::Building> {
    use super::types::{Building, HeightSource, OsmKind};
    let raw: Vec<GoldenBuilding> = load("buildings.json");
    raw.iter()
        .map(|b| Building {
            key: b.key.clone(),
            osm_id: b.osm_id,
            kind: OsmKind::from_str_or_default(&b.kind),
            ring: b.ring.clone(),
            holes: b.holes.clone(),
            node_ids: b.node_ids.clone(),
            tags: Default::default(),
            height_osm: b.height_osm,
            height_source: HeightSource::from_str_or_default(&b.height_source),
            min_height: b.min_height,
            target: b.target,
            member_ways: Vec::new(),
        })
        .collect()
}

// --------------------------------------------------------------------------- align

/// The align stage's fixtures, `tests/golden/facade/align/`.
///
/// `candidates.json` is the run's own `views.json` reshaped: every wall that has
/// candidates, every candidate with the value and verdict of the gates this
/// stage adds, and the pano ids the run selected in order. A candidate that a
/// geometric gate had already rejected carries only its reason, because the
/// geometric gates have `candidates.json` at the top level to themselves.
///
/// `cameras.json` is every camera with the shift the run gave it, so the port
/// rebuilds the registered cameras the LOS rays start from. `reg.json` plus
/// `points/` carry the registration input that cannot be recomputed without the
/// cluster clouds.
pub fn align_dir() -> PathBuf {
    dir().join("align")
}

#[derive(Debug, Deserialize)]
pub struct GoldenAlignFile {
    pub gate_names: Vec<String>,
    pub walls: Vec<GoldenAlignWall>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenAlignWall {
    pub wall_key: String,
    pub building_key: String,
    pub reachable: bool,
    pub unreachable_reason: String,
    pub single_view: bool,
    pub n_los_clean: usize,
    /// The pano ids the run selected, best first.
    pub selected: Vec<String>,
    pub candidates: Vec<GoldenAlignCandidate>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenAlignCandidate {
    pub pano_id: String,
    pub rejected_reason: Option<String>,
    /// The rest is only on the candidates that reached the LOS test.
    pub visible_frac: Option<f64>,
    pub s_vis: Option<[f64; 2]>,
    pub f_occ: Option<f64>,
    pub blur: Option<f64>,
    pub score: Option<f64>,
    pub g_score: Option<f64>,
    #[serde(default)]
    pub gate_pass: Vec<Option<u8>>,
    #[serde(default)]
    pub gate_value: Vec<Option<f64>>,
}

impl GoldenAlignFile {
    /// The value and verdict of one gate on a candidate, if it has one.
    pub fn gate(&self, cand: &GoldenAlignCandidate, name: &str) -> Option<(bool, f64)> {
        let i = self.gate_names.iter().position(|n| n == name)?;
        match (cand.gate_pass.get(i)?, cand.gate_value.get(i)?) {
            (Some(p), Some(v)) => Some((*p != 0, *v)),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GoldenAlignCamera {
    pub pano_id: String,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub is_spherical: bool,
    pub pose_factor: f64,
    pub reg_source: String,
    /// `(dx, dy, theta_deg)`, zero when the registration was not accepted.
    pub shift: [f64; 3],
}

#[derive(Debug, Deserialize)]
pub struct GoldenRegFile {
    pub bbox_xy: [f64; 4],
    pub camera_xy: Vec<[f64; 2]>,
    pub dt: GoldenRegDt,
    pub panos: Vec<GoldenRegPano>,
    pub clusters: Vec<GoldenRegCluster>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRegDt {
    pub res_m: f64,
    pub margin_m: f64,
    pub n: usize,
    pub origin: [f64; 2],
    pub size_m: f64,
    pub outline_cells: usize,
    /// `(x, y, distance)` lookups of the run's own transform.
    pub samples: Vec<[f64; 3]>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRegPano {
    pub pano_id: String,
    pub cluster_id: String,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub ground_z: f64,
    pub cam_height_m: f64,
    pub points_file: String,
    /// The whole band, which is what the search runs on.
    pub points: usize,
    pub band_points: usize,
    pub local: GoldenRegResult,
    /// What the pano ended up with after the cluster-wide fallback.
    #[serde(rename = "final")]
    pub outcome: GoldenRegResult,
    /// Only on the panos whose band is bigger than the old 4000 point cap: what
    /// a subsample of that size would have decided instead, which is the size
    /// of the effect the cap used to have on the verdict.
    pub sampled: Option<GoldenRegResult>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRegResult {
    pub dx: f64,
    pub dy: f64,
    pub theta_deg: f64,
    pub accepted: bool,
    pub source: String,
    pub inliers_before: Option<f64>,
    pub inliers_after: Option<f64>,
    pub ambiguity_ratio: Option<f64>,
    pub radius_agreement_m: Option<f64>,
    pub n_points: Option<usize>,
    pub reason: Option<String>,
    pub second_best: Option<Vec<f64>>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRegCluster {
    pub cluster_id: String,
    pub z_g: f64,
    pub band_points: usize,
    pub points_file: String,
    pub points: usize,
    /// `(dx, dy, inlier fraction)`, or `None` when the fallback was refused.
    pub shift: Option<[f64; 3]>,
}

/// One point file: little endian `f32` xy pairs in the run frame.
pub fn align_points(file: &str) -> Vec<[f64; 2]> {
    let path = align_dir().join(file);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    bytes
        .chunks_exact(8)
        .map(|c| {
            let f = |k: usize| {
                f64::from(f32::from_le_bytes(
                    c[k * 4..k * 4 + 4].try_into().expect("4 bytes"),
                ))
            };
            [f(0), f(1)]
        })
        .collect()
}

/// One preview crop with everything it was rendered from.
#[derive(Debug, Deserialize)]
pub struct GoldenCrop {
    pub view_key: String,
    pub wall_key: String,
    pub pano_id: String,
    pub camera: GoldenCropCamera,
    /// The wall already moved onto its fitted plane.
    pub wall: GoldenCropWall,
    pub z_base: f64,
    pub s_vis: [f64; 2],
    pub meta: GoldenPreviewMeta,
    pub rows: u32,
    pub width: u32,
    pub depth_file: Option<String>,
    pub depth_size: [u32; 2],
    /// Pixels carrying each occlusion bit, so a failure says which plane moved.
    pub bits: GoldenCropBits,
    /// Gate name to `(passed, value)`.
    pub gates: std::collections::BTreeMap<String, (bool, f64)>,
    pub z_base_source: String,
    pub rejected_reason: Option<String>,
    pub plane_source: String,
    pub image_file: String,
    /// Only the few records that also carry their pixels.
    pub veg_pixels: Option<usize>,
    pub rgb_file: Option<String>,
    pub occl_file: Option<String>,
    pub veg_file: Option<String>,
}

/// The pixel count of every occlusion bit of one crop.
#[derive(Debug, Deserialize)]
pub struct GoldenCropBits {
    pub nadir: usize,
    pub zenith: usize,
    pub outside: usize,
    pub footprint: usize,
    pub cloud: usize,
    pub seg: usize,
}

#[derive(Debug, Deserialize)]
pub struct GoldenCropCamera {
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub camera_type: String,
    pub width: u32,
    pub height: u32,
    pub camera_params: Option<Vec<f64>>,
    pub ground_z: f64,
    pub cam_height_m: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenCropWall {
    pub key: String,
    pub building_key: String,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub n: [f64; 2],
    pub length: f64,
    pub height_osm: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenPreviewMeta {
    pub ppm: f64,
    pub ppm_v: f64,
    pub s0: f64,
    pub s1: f64,
    pub h_top: f64,
    pub h_bot: f64,
    pub x_foot: f64,
    pub y_cam: f64,
}

impl GoldenCrop {
    pub fn camera(&self) -> super::types::Camera {
        use super::types::{Camera, CameraModel, GroundSource, PoseSource};
        Camera {
            pano_id: self.pano_id.clone(),
            centre: self.camera.centre,
            axes: self.camera.axes,
            pose_source: PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: self.camera.ground_z,
            cam_height_m: self.camera.cam_height_m,
            ground_source: GroundSource::Cloud,
            cluster_id: None,
            shot_id: None,
            reg: None,
            compass_deg: 0.0,
            pose_factor: 1.0,
            width: self.camera.width,
            height: self.camera.height,
            camera_type: CameraModel::parse(&self.camera.camera_type),
            camera_params: self.camera.camera_params.clone().unwrap_or_default(),
        }
    }

    /// The wall on its fitted plane, which is what the crop was rendered on.
    pub fn wall(&self) -> super::types::Wall {
        use super::types::{HeightSource, Wall};
        Wall {
            key: self.wall.key.clone(),
            building_key: self.wall.building_key.clone(),
            idx: 0,
            node_a: 0,
            node_b: 0,
            a: self.wall.a,
            b: self.wall.b,
            n: self.wall.n,
            length: self.wall.length,
            merged_idx: vec![],
            piece: 0,
            n_pieces: 1,
            height_osm: self.wall.height_osm,
            height_source: HeightSource::Tag,
            reachable: true,
            unreachable_reason: String::new(),
            edges: vec![],
            s_offset: 0.0,
        }
    }

    fn open(&self, file: &str) -> image::DynamicImage {
        let path = align_dir().join(file);
        image::open(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    pub fn source_image(&self) -> image::RgbImage {
        self.open(&self.image_file).to_rgb8()
    }

    pub fn rgb(&self) -> Option<image::RgbImage> {
        Some(self.open(self.rgb_file.as_ref()?).to_rgb8())
    }

    pub fn occl(&self) -> Option<Vec<u8>> {
        Some(
            self.open(self.occl_file.as_ref()?)
                .to_luma8()
                .pixels()
                .map(|p| p.0[0])
                .collect(),
        )
    }

    pub fn veg(&self) -> Option<Vec<bool>> {
        Some(
            self.open(self.veg_file.as_ref()?)
                .to_luma8()
                .pixels()
                .map(|p| p.0[0] > 0)
                .collect(),
        )
    }

    pub fn depth(&self) -> Option<super::sfm::DepthMap> {
        let file = self.depth_file.as_ref()?;
        let img = self.open(file).to_luma16();
        Some(super::sfm::DepthMap {
            width: img.width(),
            height: img.height(),
            depth: img.pixels().map(|p| f16_bits_to_f32(p.0[0])).collect(),
        })
    }
}

pub fn crops() -> Vec<GoldenCrop> {
    load("align/crops.json")
}

/// Every camera of the box with the registration the run gave it applied, which
/// is what the align stage's own gates run on.
pub fn registered_cameras() -> std::collections::BTreeMap<String, super::types::Camera> {
    use super::types::{RegResult, RegSource};
    let mut cams = geometry_cameras();
    let regs: Vec<GoldenAlignCamera> = load("align/cameras.json");
    for r in &regs {
        let Some(cam) = cams.get_mut(&r.pano_id) else {
            continue;
        };
        let source = RegSource::from_str_or_default(&r.reg_source);
        let reg = RegResult {
            dx: r.shift[0],
            dy: r.shift[1],
            theta_deg: r.shift[2],
            accepted: source == RegSource::Local || source == RegSource::Global,
            source,
            ..RegResult::default()
        };
        *cam = super::register::apply_registration(cam, &reg);
    }
    cams
}

// --------------------------------------------------------------------------- refine

/// The refine stage's fixtures, `tests/golden/facade/refine/`.
///
/// Two layers: a handful of sampled views carry their loose crop as two PNGs
/// (the RGB and the occlusion byte plane) with every product of
/// `refine.refine_view`, and `walls.json` carries, for every wall of the run
/// that has views, what `decide_wall` consumes and produces. The height in it
/// is the fixed roof vote; `reference.h_used` is what the run itself decided
/// with the old one.
///
/// Regenerate with:
///
/// ```text
/// set PYTHONIOENCODING=utf-8
/// python tools/facade_lab/export_golden_refine.py --run munich_plane
/// ```
pub fn refine_dir() -> PathBuf {
    dir().join("refine")
}

#[derive(Debug, Deserialize)]
pub struct GoldenRefineManifest {
    pub run: String,
    pub tolerances: GoldenRefineTolerances,
    pub views: Vec<GoldenRefineViewEntry>,
    pub walls_with_crops: Vec<String>,
    pub height_fix_moved: Vec<GoldenHeightMove>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRefineTolerances {
    pub lean_deg: f64,
    pub height_m: f64,
    pub extent_m: f64,
    pub profile_peak_m: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenHeightMove {
    pub wall_key: String,
    pub was: f64,
    pub now: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRefineViewEntry {
    pub view_key: String,
    pub width: u32,
    pub height: u32,
    pub lean_flag: String,
    pub roof_flag: String,
    pub ground_flag: String,
    pub plane_flag: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenCropMeta {
    pub width: u32,
    pub height: u32,
    pub ppm: f64,
    pub s0: f64,
    pub h_bot: f64,
    pub h_top: f64,
    pub x_foot: f64,
    pub y_cam: f64,
    pub z_base: f64,
    pub z_base_source: String,
    pub source_image: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenViewInput {
    #[serde(rename = "L_fit")]
    pub l_fit: f64,
    pub h_osm: f64,
    pub height_osm: Option<f64>,
    pub height_source: String,
    pub s_vis: [f64; 2],
    pub h_cloud_raw: Option<f64>,
    pub ground_source: String,
    pub score: f64,
    pub ds_m: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenLean {
    pub c0_deg: f64,
    pub c1_deg_per_m: f64,
    pub flag: String,
    pub n: usize,
    pub inliers: f64,
    pub rms_deg: f64,
    pub x_spread_m: f64,
    pub c0_after_deg: f64,
    #[serde(rename = "H_shear")]
    pub h_shear: [[f64; 3]; 3],
    pub grad_c0_deg: f64,
    pub grad_c1_deg_per_m: f64,
    pub grad_support: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenLines {
    pub n_vertical: usize,
    pub n_horizontal: usize,
}

#[derive(Debug, Deserialize)]
pub struct GoldenPlaneGate {
    pub slope_deg_per_m: f64,
    pub flag: String,
    pub n: usize,
    pub box_px: [f64; 4],
}

#[derive(Debug, Deserialize)]
pub struct GoldenGround {
    pub dz_m: f64,
    pub flag: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRoof {
    pub h_sky: Option<f64>,
    pub h_sky_raw: Option<f64>,
    pub flag: String,
    pub flags: Vec<String>,
    pub spread_m: f64,
    pub sky_mode: String,
    pub h_cloud: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenPeak {
    pub x_m: f64,
    #[serde(rename = "E")]
    pub energy: f64,
    pub vlen_m: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenProfile {
    pub ppm: f64,
    pub x0_m: f64,
    #[serde(rename = "E")]
    pub energy: Vec<f32>,
    pub peaks: Vec<GoldenPeak>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRefineView {
    pub view_key: String,
    pub wall_key: String,
    pub pano_id: String,
    pub crop: GoldenCropMeta,
    pub input: GoldenViewInput,
    pub lean: GoldenLean,
    pub lines: GoldenLines,
    pub plane: GoldenPlaneGate,
    pub ground: GoldenGround,
    pub roof: GoldenRoof,
    pub profile: GoldenProfile,
    pub z_base_corrected: f64,
}

impl GoldenRefineView {
    /// The loose crop this view was refined from.
    pub fn crop(&self) -> super::rectify::LooseCrop {
        let rgb = image::open(refine_dir().join(format!("{}_rgb.png", self.view_key)))
            .unwrap_or_else(|e| panic!("cannot read the crop of {}: {e}", self.view_key))
            .to_rgb8();
        let occl = image::open(refine_dir().join(format!("{}_occl.png", self.view_key)))
            .unwrap_or_else(|e| panic!("cannot read the occlusion of {}: {e}", self.view_key))
            .to_luma8();
        assert_eq!(
            (rgb.width(), rgb.height()),
            (self.crop.width, self.crop.height),
            "{}: the crop does not match its record",
            self.view_key
        );
        super::rectify::LooseCrop {
            wall_key: self.wall_key.clone(),
            pano_id: self.pano_id.clone(),
            rgb,
            occl: occl.pixels().map(|p| p.0[0]).collect(),
            ppm: self.crop.ppm,
            s0: self.crop.s0,
            h_bot: self.crop.h_bot,
            h_top: self.crop.h_top,
            x_foot: self.crop.x_foot,
            y_cam: self.crop.y_cam,
            z_base: self.crop.z_base,
            z_base_source: self.crop.z_base_source.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GoldenWallViewInput {
    pub pano_id: String,
    pub score: f64,
    pub ds_m: f64,
    pub h_cloud: Option<f64>,
    pub roof_flag: String,
    pub h_sky: Option<f64>,
    pub roof_flags: Vec<String>,
    pub ground_dz: f64,
    pub lean_flag: String,
    pub plane_flag: String,
    pub ground_flag: String,
    pub profile_x0_m: f64,
    #[serde(rename = "profile_E")]
    pub profile: Vec<f32>,
    pub peaks: Vec<GoldenPeak>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenWallInput {
    pub length_m: f64,
    #[serde(rename = "L_fit")]
    pub l_fit: f64,
    pub height_osm: Option<f64>,
    pub height_source: String,
    pub fit_src_a: String,
    pub fit_src_b: String,
    pub views: Vec<GoldenWallViewInput>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenWallHeight {
    pub h_sky: Option<f64>,
    pub h_cloud: Option<f64>,
    pub h_osm: Option<f64>,
    pub h_used: f64,
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenWallExtent {
    pub s_l: f64,
    pub s_r: f64,
    pub src_a: String,
    pub src_b: String,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenWallReference {
    pub h_used: Option<f64>,
    pub height_source: Option<String>,
    pub s_l: Option<f64>,
    pub s_r: Option<f64>,
    pub lean_deg: Option<f64>,
    pub tier: Option<String>,
    pub cols: Option<usize>,
    pub rows: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenRefineWall {
    pub wall_key: String,
    pub input: GoldenWallInput,
    pub height: GoldenWallHeight,
    pub extent_nocrop: GoldenWallExtent,
    pub flags: Vec<String>,
    pub reference: GoldenWallReference,
    /// Only on the walls whose crops travel with the fixture.
    #[serde(default)]
    pub extent_crop: Option<GoldenWallExtent>,
}

pub fn refine_manifest() -> GoldenRefineManifest {
    load("refine/manifest.json")
}

/// Every sampled view, in manifest order.
pub fn refine_views() -> Vec<GoldenRefineView> {
    refine_manifest()
        .views
        .iter()
        .map(|e| load(&format!("refine/{}.json", e.view_key)))
        .collect()
}

/// Every wall of the reference run that has views.
pub fn refine_walls() -> Vec<GoldenRefineWall> {
    load("refine/walls.json")
}

// --------------------------------------------------------------------------- rectify and fuse

/// The texture stage's fixtures, `tests/golden/facade/texture/`.
///
/// Two layers, for the same reason the refine fixture has two: the photographs
/// are the expensive part of this stage. Every wall carries the per view
/// textures the run rendered, the fused texture it made of them and what
/// `fuse.py` decided, which is kilobytes and covers the whole fusion. The walls
/// whose photographs are 2048 px thumbnails additionally carry the photograph,
/// the cluster depth map and the run's loose crop, so the whole stage runs from
/// the image on those. The wall list is the opening fixture's, so a fused
/// texture can be fed straight into `openings::classify` and compared with the
/// class grid the run produced.
///
/// Regenerate with:
///
/// ```text
/// set PYTHONIOENCODING=utf-8
/// python tools/facade_lab/export_golden_texture.py --run munich_plane
/// ```
pub fn texture_dir() -> PathBuf {
    dir().join("texture")
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureManifest {
    pub run: String,
    pub tolerances: GoldenTextureTolerances,
    pub walls: Vec<GoldenTextureEntry>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureTolerances {
    pub texture_mean_abs_diff: f64,
    pub valid_iou: f64,
    pub class_grid_agreement: f64,
    pub agreement_m: f64,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureEntry {
    pub key: String,
    pub tier: String,
    pub cols: usize,
    pub rows: usize,
    pub views: usize,
    pub mode: String,
    pub agreement_m: f64,
    pub hole_fraction: f64,
    pub from_image: bool,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureFit {
    pub n: [f64; 2],
    pub d: f64,
    pub source: String,
    pub a_ref: Option<[f64; 2]>,
    pub b_ref: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureCamera {
    pub pano_id: String,
    #[serde(rename = "C")]
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub ground_z: f64,
    pub cam_height_m: f64,
    pub cluster_id: Option<String>,
    pub width: u32,
    pub height: u32,
    pub camera_type: String,
    pub camera_params: Vec<f64>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureView {
    pub pano_id: String,
    pub score: f64,
    pub source_image: String,
    pub ds_m: f64,
    pub dz_m: f64,
    pub z_base: f64,
    pub dist_m: f64,
    pub s_vis: [f64; 2],
    #[serde(rename = "H_shear")]
    pub h_shear: Option<[[f64; 3]; 3]>,
    pub fit: GoldenTextureFit,
    pub cam: GoldenTextureCamera,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureFuse {
    pub mode: String,
    pub agreement_m: f64,
    pub shifts_m: Vec<[f64; 2]>,
    pub hole_fraction: f64,
    pub best_index: usize,
    pub flags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureCrop {
    pub width: u32,
    pub height: u32,
    pub ppm: f64,
    pub s0: f64,
    pub h_bot: f64,
    pub h_top: f64,
    pub x_foot: f64,
    pub y_cam: f64,
    pub z_base: f64,
    pub z_base_source: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureRectify {
    pub pano_id: String,
    /// The photograph, where it travels with the fixture.
    pub image: Option<String>,
    /// Its path inside `cache_munich_test`, for the lab measurement.
    pub cache_image: String,
    pub depth: Option<String>,
    /// The loose crop the run wrote, where it travels with the fixture.
    pub crop: Option<GoldenTextureCrop>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTextureWall {
    pub key: String,
    pub tier: String,
    pub cols: usize,
    pub rows: usize,
    pub ppb: u32,
    pub rect: [f64; 4],
    pub height_used_m: f64,
    pub views: Vec<GoldenTextureView>,
    pub fuse: GoldenTextureFuse,
    #[serde(default)]
    pub rectify: Option<Vec<GoldenTextureRectify>>,
}

/// An RGBA fixture texture as `(rgb, valid)`.
pub fn load_rgba(path: &Path) -> (image::RgbImage, Vec<bool>) {
    let img = image::open(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
        .to_rgba8();
    let rgb = image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        image::Rgb([p[0], p[1], p[2]])
    });
    let valid = img.pixels().map(|p| p.0[3] > 0).collect();
    (rgb, valid)
}

impl GoldenTextureView {
    pub fn camera(&self) -> super::types::Camera {
        use super::types::{Camera, CameraModel, GroundSource, PoseSource};
        Camera {
            pano_id: self.cam.pano_id.clone(),
            centre: self.cam.centre,
            axes: self.cam.axes,
            pose_source: PoseSource::Sfm,
            roll_deg: 0.0,
            pitch_deg: 0.0,
            ground_z: self.cam.ground_z,
            cam_height_m: self.cam.cam_height_m,
            ground_source: GroundSource::Cloud,
            cluster_id: self.cam.cluster_id.clone(),
            shot_id: None,
            reg: None,
            compass_deg: 0.0,
            pose_factor: 1.0,
            width: self.cam.width,
            height: self.cam.height,
            camera_type: CameraModel::parse(&self.cam.camera_type),
            camera_params: self.cam.camera_params.clone(),
        }
    }

    pub fn plane_fit(&self, wall_key: &str) -> super::types::PlaneFit {
        use super::types::{PlaneFit, PlaneSource};
        PlaneFit {
            wall_key: wall_key.to_string(),
            n: self.fit.n,
            d: self.fit.d,
            source: PlaneSource::from_str_or_default(&self.fit.source),
            n_inliers: 0,
            pts_per_m: 0.0,
            rms_m: 0.0,
            angle_vs_osm_deg: 0.0,
            offset_vs_osm_m: 0.0,
            z_top98: None,
            z_continuous: false,
            ambiguity: 0.0,
            a_ref: self.fit.a_ref,
            b_ref: self.fit.b_ref,
            pano_id: Some(self.pano_id.clone()),
            cluster_id: self.cam.cluster_id.clone(),
        }
    }
}

impl GoldenTextureWall {
    /// The rectangle this view renders: the wall's own, moved by the view's
    /// along-wall offset and the wall's ground correction.
    pub fn view_rect(&self, view: &GoldenTextureView) -> [f64; 4] {
        [
            self.rect[0] - view.ds_m,
            self.rect[1] - view.ds_m,
            self.rect[2] + view.dz_m,
            self.rect[3] + view.dz_m,
        ]
    }

    /// One view's texture as the run rendered it.
    pub fn view_texture(&self, view: &GoldenTextureView) -> super::fuse::ViewTexture {
        let path = texture_dir().join(format!("{}__{}.png", self.key, view.pano_id));
        let (rgb, valid) = load_rgba(&path);
        super::fuse::ViewTexture {
            pano_id: view.pano_id.clone(),
            rgb,
            valid,
            score: view.score,
        }
    }

    pub fn view_textures(&self) -> Vec<super::fuse::ViewTexture> {
        self.views.iter().map(|v| self.view_texture(v)).collect()
    }

    /// The fused texture the run wrote.
    pub fn fused(&self) -> (image::RgbImage, Vec<bool>) {
        load_rgba(&texture_dir().join(format!("{}_tex.png", self.key)))
    }

    /// The photograph one view was rendered from, where it travels with the
    /// fixture.
    pub fn image(&self, r: &GoldenTextureRectify) -> Option<image::RgbImage> {
        let name = r.image.as_ref()?;
        let path = texture_dir().join(name);
        Some(
            image::open(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
                .to_rgb8(),
        )
    }

    /// The same photograph out of the Python lab's own cache, which is where
    /// the originals live. `None` when the lab is not on this machine.
    pub fn lab_image(&self, r: &GoldenTextureRectify) -> Option<image::RgbImage> {
        let path = lab_cache_dir()?.join(&r.cache_image);
        if !path.exists() {
            return None;
        }
        Some(
            image::open(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
                .to_rgb8(),
        )
    }

    /// The cluster depth map of one view, where there is one.
    pub fn depth(&self, r: &GoldenTextureRectify) -> Option<super::sfm::DepthMap> {
        let name = r.depth.as_ref()?;
        let img = image::open(texture_dir().join(name))
            .unwrap_or_else(|e| panic!("cannot read {name}: {e}"))
            .to_luma16();
        Some(super::sfm::DepthMap {
            width: img.width(),
            height: img.height(),
            depth: img.pixels().map(|p| f16_bits_to_f32(p.0[0])).collect(),
        })
    }

    /// The loose crop the run wrote for one view, where it travels with the
    /// fixture.
    pub fn crop(&self, r: &GoldenTextureRectify) -> Option<super::rectify::LooseCrop> {
        let meta = r.crop.as_ref()?;
        let stem = format!("{}__{}", self.key, r.pano_id);
        let rgb = image::open(texture_dir().join(format!("{stem}_crop_rgb.png")))
            .unwrap_or_else(|e| panic!("cannot read the crop of {stem}: {e}"))
            .to_rgb8();
        let occl = image::open(texture_dir().join(format!("{stem}_crop_occl.png")))
            .unwrap_or_else(|e| panic!("cannot read the occlusion of {stem}: {e}"))
            .to_luma8();
        assert_eq!(
            (rgb.width(), rgb.height()),
            (meta.width, meta.height),
            "{stem}: the crop does not match its record"
        );
        Some(super::rectify::LooseCrop {
            wall_key: self.key.clone(),
            pano_id: r.pano_id.clone(),
            rgb,
            occl: occl.pixels().map(|p| p.0[0]).collect(),
            ppm: meta.ppm,
            s0: meta.s0,
            h_bot: meta.h_bot,
            h_top: meta.h_top,
            x_foot: meta.x_foot,
            y_cam: meta.y_cam,
            z_base: meta.z_base,
            z_base_source: meta.z_base_source.clone(),
        })
    }
}

/// The Python lab's own cache, which holds the original photographs the
/// reference run rendered from. It is not part of the fixture and not on every
/// machine, so everything that reads it says so and steps aside when it is
/// missing.
pub fn lab_cache_dir() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("facade_lab")
        .join("cache_munich_test");
    p.is_dir().then_some(p)
}

pub fn texture_manifest() -> GoldenTextureManifest {
    load("texture/manifest.json")
}

/// Every wall of the texture fixture, in manifest order.
pub fn texture_walls() -> Vec<GoldenTextureWall> {
    texture_manifest()
        .walls
        .iter()
        .map(|e| load(&format!("texture/{}.json", e.key)))
        .collect()
}

/// The mean absolute difference over the texels both masks call valid, and the
/// intersection over union of the two masks. The pair the texture tolerances
/// are stated in.
pub fn texture_agreement(
    a: &image::RgbImage,
    a_valid: &[bool],
    b: &image::RgbImage,
    b_valid: &[bool],
) -> (f64, f64) {
    assert_eq!(a.dimensions(), b.dimensions(), "textures differ in size");
    let n = a_valid.len();
    let (mut sum, mut count) = (0.0f64, 0usize);
    let (mut inter, mut union) = (0usize, 0usize);
    for i in 0..n {
        if a_valid[i] && b_valid[i] {
            inter += 1;
            for c in 0..3 {
                sum += (f64::from(a.as_raw()[i * 3 + c]) - f64::from(b.as_raw()[i * 3 + c])).abs();
            }
            count += 3;
        }
        if a_valid[i] || b_valid[i] {
            union += 1;
        }
    }
    let mad = if count == 0 { 0.0 } else { sum / count as f64 };
    let iou = if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    };
    (mad, iou)
}
