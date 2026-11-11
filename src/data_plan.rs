//! What a run of the selected area would download, read off the caches on
//! disk alone: no request is made. The window shows it before an offline run.
use crate::args::Args;
use crate::coordinate_system::geographic::LLBBox;
use std::path::{Path, PathBuf};

/// One data source of the run.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Item {
    /// `osm`, `elevation`, `land_cover` or `overture`.
    pub source: &'static str,
    pub cached: usize,
    /// What the cached files take on disk.
    pub cached_bytes: u64,
    /// Files the run reads; 0 when nothing can be counted (Overpass, an area
    /// past what the source reads tile by tile, or no index on disk yet).
    pub total: usize,
    /// Estimated download for the files not cached yet.
    pub missing_bytes: Option<u64>,
}

#[derive(serde::Serialize, Debug, Default)]
pub struct DataPlan {
    /// The folder every cache sits under.
    pub cache_root: String,
    pub items: Vec<Item>,
    /// The `.pbf` extract, with Region Download or a `.pbf` file.
    pub extract: Option<crate::osm_pbf::ExtractPlan>,
    /// The local tile archive folder's archives, with Local Archive.
    pub local_archive: Option<crate::osm_tiles::LocalCoverage>,
    /// Free space on the disk the caches are on.
    pub free_bytes: Option<u64>,
}

// Bake sizes, as multiples of the .pbf they come from. Measured 2026-10 on
// Geofabrik extracts (docs/11, work/p4-archive, the p6-bake measurements).

/// A Region Download bake of a whole extract: Liechtenstein 3.46 MB to a
/// 6.87 MB .json.zst. A selection bakes its share of the extract's area.
const REGION_BAKE_PER_PBF: f64 = 2.0;
/// Peak memory of a Region Download bake: 4-5x (Romania 330 MB to 1.3-1.6 GB).
const BAKE_RAM_PER_PBF: f64 = 4.5;
/// An arnis-tiles archive: Switzerland 0.58, Liechtenstein 0.63, Austria
/// 0.67, Romania 1.39. The estimate takes a typical one; the room check the
/// largest.
pub const ARCHIVE_PER_PBF: f64 = 0.7;
const ARCHIVE_PER_PBF_MAX: f64 = 1.4;
/// arnis-tiles' chunk store while it bakes: Switzerland 0.81, Austria 0.92,
/// Romania 1.73.
pub const STORE_PER_PBF: f64 = 0.9;
const STORE_PER_PBF_MAX: f64 = 1.75;

fn times(bytes: u64, ratio: f64) -> u64 {
    (bytes as f64 * ratio).round() as u64
}

/// The Region Download bake of `share` (0-1) of an extract of `pbf` bytes.
pub fn region_bake_bytes(pbf: u64, share: f64) -> u64 {
    times(pbf, REGION_BAKE_PER_PBF * share.clamp(0.0, 1.0))
}

/// Memory a Region Download bake of an extract of `pbf` bytes peaks at.
pub fn bake_ram_bytes(pbf: u64) -> u64 {
    times(pbf, BAKE_RAM_PER_PBF)
}

/// The archive arnis-tiles bakes from an extract of `pbf` bytes.
pub fn archive_bytes(pbf: u64) -> u64 {
    times(pbf, ARCHIVE_PER_PBF)
}

/// The most disk a country bake holds at once for extracts of these sizes,
/// in arnis-tiles' order (largest first): the window downloads them all
/// first, then each one's chunk store grows beside them, its .pbf goes, its
/// archive is written and the store goes. Taken at the largest measured
/// ratios, as it decides whether the disk is big enough.
pub fn archive_peak_bytes(pbfs: &[u64]) -> u64 {
    let mut sorted = pbfs.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let (mut done, mut peak) = (0u64, 0u64);
    for (i, &pbf) in sorted.iter().enumerate() {
        let later: u64 = sorted[i + 1..].iter().sum();
        let store = times(pbf, STORE_PER_PBF_MAX);
        let archive = times(pbf, ARCHIVE_PER_PBF_MAX);
        peak = peak.max(done + later + store + pbf.max(archive));
        done += archive;
    }
    peak
}

/// Free space on the disk holding `path`, or its nearest existing parent.
pub fn free_bytes(path: &Path) -> Option<u64> {
    path.ancestors()
        .find(|p| p.exists())
        .and_then(|p| fs2::available_space(p).ok())
}

/// Typical cached file per source, measured over a working cache (2026-10),
/// for the estimate while none of the area is cached yet; afterwards the
/// area's own files set it.
const TYPICAL_OSM_TILE: u64 = 240_000;
const TYPICAL_ELEVATION_TILE: u64 = 72_000;
const TYPICAL_LAND_COVER_TILE: u64 = 38_000;
const TYPICAL_OVERTURE_TILE: u64 = 142_000;

fn count(source: &'static str, files: Option<Vec<PathBuf>>, typical: u64) -> Item {
    let files = files.unwrap_or_default();
    let (mut cached, mut bytes) = (0usize, 0u64);
    for f in &files {
        if let Ok(m) = std::fs::metadata(f) {
            cached += 1;
            bytes += m.len();
        }
    }
    let per = if cached > 0 {
        bytes / cached as u64
    } else {
        typical
    };
    Item {
        source,
        cached,
        cached_bytes: bytes,
        total: files.len(),
        missing_bytes: (!files.is_empty()).then(|| per * (files.len() - cached) as u64),
    }
}

/// The plan for a run of `bbox` with `args`, from the caches under `root`.
pub fn plan(root: &Path, args: &Args, bbox: LLBBox) -> DataPlan {
    let (_, _, grid_w, grid_h) = crate::elevation::compute_grid_dims(&bbox, args.scale);
    let earth = args.body.is_earth();
    let mut out = DataPlan {
        cache_root: root.display().to_string(),
        free_bytes: free_bytes(root),
        ..DataPlan::default()
    };
    if !args.skip_objects() {
        if let Some(src) = crate::osm_pbf::Source::from_args(args) {
            let e = crate::osm_pbf::plan(root, &src, bbox);
            out.items.push(Item {
                source: "osm",
                cached: usize::from(e.downloaded) + usize::from(e.baked),
                cached_bytes: e.bytes.unwrap_or(0) + e.bake_bytes.unwrap_or(0),
                total: 2,
                // A bake is cut locally; only the extract downloads, and its
                // size is known once on disk or once a download or HEAD saw it.
                missing_bytes: if e.downloaded {
                    Some(0)
                } else {
                    e.download_bytes
                },
            });
            out.extract = Some(e);
        } else if let Some(file) = &args.file {
            let item = count("osm", Some(vec![PathBuf::from(file)]), 0);
            out.items.push(Item {
                missing_bytes: None,
                ..item
            });
        } else if args.no_tile_archive {
            out.items.push(count("osm", None, 0));
        } else if let Some(dir) = crate::overture::pmtiles::local_path(&args.osm_tiles_url) {
            // A local archive is read in place: a tile is there when an
            // archive in the folder holds its cell, and nothing downloads.
            let cov = crate::osm_tiles::local_coverage(&dir, &bbox);
            out.items.push(Item {
                source: "osm",
                cached: cov.as_ref().map_or(0, |c| c.covered),
                cached_bytes: cov.as_ref().map_or(0, |c| {
                    c.archives
                        .iter()
                        .filter(|a| a.covers)
                        .filter_map(|a| a.bytes)
                        .sum()
                }),
                total: cov.as_ref().map_or(0, |c| c.tiles),
                missing_bytes: None,
            });
            out.local_archive = cov;
        } else {
            let files = crate::osm_tiles::cache_files(root, &args.osm_tiles_url, &bbox);
            out.items.push(count("osm", files, TYPICAL_OSM_TILE));
        }
    }
    if args.terrain() && earth {
        let files = if args.aws_only_elevation {
            let dir = crate::elevation::cache::provider_cache_dir(root, "aws");
            crate::elevation::providers::aws_terrain::cache_files(&dir, &bbox)
        } else {
            let dir = crate::elevation::cache::provider_cache_dir(root, "mapterhorn");
            crate::elevation::providers::mapterhorn::cache_files(&dir, &bbox, grid_w, grid_h)
        };
        out.items
            .push(count("elevation", Some(files), TYPICAL_ELEVATION_TILE));
    }
    if earth {
        let files = crate::land_cover::cache_files(root, &bbox);
        out.items
            .push(count("land_cover", Some(files), TYPICAL_LAND_COVER_TILE));
    }
    // ponytail: the canopy strip table and row blocks are not listed, so it has no item
    // (an item with total 0 says nothing); list them to add one.
    if args.overture && !args.skip_objects() {
        let files = crate::overture::tile_cache_files(root, &bbox);
        out.items
            .push(count("overture", files, TYPICAL_OVERTURE_TILE));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    const BBOX: &str = "44.55,26.00,44.555,26.008";

    fn args(extra: &[&str]) -> Args {
        let bbox = format!("--bbox={BBOX}");
        Args::try_parse_from(["arnis", bbox.as_str()].iter().chain(extra)).unwrap()
    }

    fn item<'a>(p: &'a DataPlan, source: &str) -> &'a Item {
        p.items.iter().find(|i| i.source == source).unwrap()
    }

    fn touch(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn counts_what_a_temp_cache_root_holds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let bbox = LLBBox::from_str(BBOX).unwrap();
        let a = args(&["--canopy-height=true"]);

        let empty = plan(root, &a, bbox);
        let elev = item(&empty, "elevation");
        assert!(elev.total > 0 && elev.cached == 0);
        assert_eq!(
            elev.missing_bytes,
            Some(TYPICAL_ELEVATION_TILE * elev.total as u64)
        );
        assert!(item(&empty, "land_cover").total >= 2, "header and a tile");
        // No archive index on disk yet: nothing to count, nothing claimed.
        assert_eq!(item(&empty, "osm").total, 0);
        assert_eq!(item(&empty, "overture").total, 0, "no release on record");

        // One of each source's files cached: partly cached, and the estimate
        // follows the cached file's size.
        let (_, _, gw, gh) = crate::elevation::compute_grid_dims(&bbox, a.scale);
        let dir = crate::elevation::cache::provider_cache_dir(root, "mapterhorn");
        let elev_files = crate::elevation::providers::mapterhorn::cache_files(&dir, &bbox, gw, gh);
        touch(&elev_files[0], 1000);
        for f in crate::land_cover::cache_files(root, &bbox) {
            touch(&f, 10);
        }
        let partly = plan(root, &a, bbox);
        let elev = item(&partly, "elevation");
        assert_eq!(elev.cached, 1);
        assert_eq!(elev.missing_bytes, Some(1000 * (elev.total as u64 - 1)));
        let lc = item(&partly, "land_cover");
        assert_eq!((lc.cached, lc.missing_bytes), (lc.total, Some(0)));

        // Region Download: the extract named by the cached index, then on disk.
        let pbf = args(&["--osm-pbf=geofabrik"]);
        let index = serde_json::json!({"features": [{
            "properties": {"name": "Test Region",
                "urls": {"pbf": "https://example.invalid/test-latest.osm.pbf"}},
            "geometry": {"type": "Polygon",
                "coordinates": [[[25.0, 44.0], [27.0, 44.0], [27.0, 45.0], [25.0, 45.0], [25.0, 44.0]]]}
        }]});
        let osm_pbf = root.join("arnis").join("osm-pbf");
        std::fs::create_dir_all(&osm_pbf).unwrap();
        std::fs::write(osm_pbf.join("geofabrik-index.json"), index.to_string()).unwrap();
        let e = plan(root, &pbf, bbox).extract.unwrap();
        assert_eq!(e.name.as_deref(), Some("Test Region"));
        assert!(!e.downloaded && !e.baked && e.bytes.is_none());
        assert_eq!(
            e.url.as_deref(),
            Some("https://example.invalid/test-latest.osm.pbf")
        );
        assert_eq!((e.download_bytes, e.bake_estimate), (None, None));
        // A size a HEAD saw names the download before it happens.
        touch(&osm_pbf.join("downloads").join("sizes.json"), 0);
        std::fs::write(
            osm_pbf.join("downloads").join("sizes.json"),
            r#"{"test-latest.osm.pbf": 2000000000}"#,
        )
        .unwrap();
        let p = plan(root, &pbf, bbox);
        let e = p.extract.as_ref().unwrap();
        assert_eq!(e.download_bytes, Some(2_000_000_000));
        assert_eq!(item(&p, "osm").missing_bytes, Some(2_000_000_000));
        assert_eq!(e.ram_estimate, Some(9_000_000_000));
        // The selection is a sliver of the 2 deg² region: its bake is too.
        let bake = e.bake_estimate.unwrap();
        assert!(bake > 0 && bake < 10_000_000, "{bake}");
        touch(&osm_pbf.join("downloads").join("test-latest.osm.pbf"), 4321);
        let p = plan(root, &pbf, bbox);
        assert_eq!(p.extract.as_ref().unwrap().bytes, Some(4321));
        assert_eq!(p.extract.as_ref().unwrap().download_bytes, Some(4321));
        assert_eq!(item(&p, "osm").cached, 1, "downloaded, not baked");
        assert_eq!(item(&p, "osm").cached_bytes, 4321);
        assert!(p.free_bytes.is_some_and(|b| b > 0));

        // Local Archive: the folder's cells, nothing to download.
        let local = root.join("local-archive");
        let flag = format!("--osm-tiles-url={}", local.display());
        let empty = plan(root, &args(&[flag.as_str()]), bbox);
        assert_eq!(item(&empty, "osm").cached, 0);
        assert!(item(&empty, "osm").total > 0);
        assert!(empty.local_archive.unwrap().archives.is_empty());

        // Cached bytes are the files' own sizes.
        assert_eq!(item(&partly, "elevation").cached_bytes, 1000);

        // Terrain-only builds no objects, so OSM is not part of the plan.
        let terrain_only = plan(root, &args(&["--mode=terrain-only"]), bbox);
        assert!(terrain_only.items.iter().all(|i| i.source != "osm"));
    }

    #[test]
    fn plan_names_the_cache_root_and_skips_empty_canopy() {
        let dir = tempfile::tempdir().unwrap();
        let bbox = LLBBox::from_str(BBOX).unwrap();
        let p = plan(dir.path(), &args(&["--canopy-height=true"]), bbox);
        assert_eq!(p.cache_root, dir.path().display().to_string());
        assert!(p.items.iter().all(|i| i.source != "canopy"));
    }

    #[test]
    fn bake_sizes_follow_the_measured_ratios() {
        // Liechtenstein: a 3.46 MB extract baked whole came to 6.87 MB.
        assert_eq!(region_bake_bytes(3_463_268, 1.0), 6_926_536);
        assert_eq!(region_bake_bytes(1_000, 0.25), 500);
        assert_eq!(
            region_bake_bytes(1_000, 7.0),
            2_000,
            "never past the whole extract"
        );
        assert_eq!(bake_ram_bytes(330_000_000), 1_485_000_000);
        assert_eq!(archive_bytes(1_000_000), 700_000);
        // One extract: the .pbf and its store, or the store and the archive.
        assert_eq!(archive_peak_bytes(&[100]), 175 + 140);
        // Two: both downloaded first; the first archive then stays while the
        // second bakes (140 + 18 + 14).
        assert_eq!(archive_peak_bytes(&[10, 100]), 10 + 175 + 140);
        // Equal extracts: the archives written so far add up.
        assert_eq!(archive_peak_bytes(&[100, 100]), 140 + 175 + 140);
        assert_eq!(archive_peak_bytes(&[100, 100, 100]), 2 * 140 + 175 + 140);
        assert_eq!(archive_peak_bytes(&[]), 0);
    }
}
