use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::ElevationProvider;
use crate::elevation::providers::aws_terrain::AwsTerrain;
use crate::elevation::providers::mapterhorn::Mapterhorn;
use crate::elevation::providers::usgs_3dep::Usgs3dep;

/// How the caller wants the elevation source chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMode {
    /// Best available: regional high-res providers first, then the
    /// global Mapterhorn tiles. Used for world generation.
    Auto,
    /// Mapterhorn only, skipping the regional providers. Used by the 3D
    /// preview: globally available, CDN-fast, and it doesn't put preview
    /// load on the rate-limited regional services.
    GlobalOnly,
    /// Legacy AWS Terrain Tiles (~30 m) only. Surfaced as the
    /// `--aws-only-elevation` CLI flag / "Legacy terrain" GUI toggle —
    /// an escape hatch if the primary elevation sources are unreachable.
    AwsOnly,
}

/// Check if two EPSG:4326 bounding boxes overlap.
pub fn bboxes_overlap(a: &LLBBox, b: &LLBBox) -> bool {
    a.min().lat() <= b.max().lat()
        && a.max().lat() >= b.min().lat()
        && a.min().lng() <= b.max().lng()
        && a.max().lng() >= b.min().lng()
}

/// Select the best elevation provider for the given bounding box.
///
/// In [`SourceMode::Auto`], iterates regional providers ordered by
/// resolution (finest first) and returns the first whose coverage
/// overlaps the user's bbox and whose `accepts()` check passes.
/// Everything else falls through to Mapterhorn, which is global.
///
/// The caller (`fetch_elevation_data`) chains further fallbacks at
/// fetch time: regional failure → Mapterhorn → AWS Terrain Tiles.
pub fn select_provider(bbox: &LLBBox, mode: SourceMode) -> Box<dyn ElevationProvider> {
    match mode {
        SourceMode::AwsOnly => {
            println!("Using AWS Terrain Tiles only (legacy mode, ~30m resolution)");
            return Box::new(AwsTerrain);
        }
        SourceMode::GlobalOnly => {
            println!("Using Mapterhorn terrain tiles (global)");
            return Box::new(Mapterhorn);
        }
        SourceMode::Auto => {}
    }

    for provider in build_provider_list() {
        if let Some(coverages) = provider.coverage_bboxes() {
            if coverages.iter().any(|c| bboxes_overlap(c, bbox)) && provider.accepts(bbox) {
                println!(
                    "Selected elevation provider: {} ({:.0}m resolution)",
                    provider.name(),
                    provider.native_resolution_m()
                );
                return provider;
            }
        }
    }

    // Global default: 30 m Copernicus worldwide, national LiDAR
    // (0.25-10 m) where available.
    println!("Using Mapterhorn terrain tiles (global; high-res where available)");
    Box::new(Mapterhorn)
}

/// Regional providers that beat Mapterhorn in their coverage area,
/// ordered by resolution (finest first). First match wins.
///
/// Mapterhorn itself is NOT in this list — it's the global default.
/// Former entries for Germany (DGM1 via hoehendaten.de), IGN France,
/// IGN Spain and Japan GSI were removed because Mapterhorn ingests the
/// same upstream datasets (state DGM1s, RGE ALTI, MDT02/05, GSI DEM) at
/// equal or better resolution without their rate limits.
fn build_provider_list() -> Vec<Box<dyn ElevationProvider>> {
    vec![
        Box::new(Usgs3dep), // 1.0m — ArcGIS REST; Mapterhorn only has 10m for most of the US
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bboxes_overlap() {
        let a = LLBBox::new(0.0, 0.0, 10.0, 10.0).unwrap();
        let b = LLBBox::new(5.0, 5.0, 15.0, 15.0).unwrap();
        assert!(bboxes_overlap(&a, &b));

        let c = LLBBox::new(20.0, 20.0, 30.0, 30.0).unwrap();
        assert!(!bboxes_overlap(&a, &c));
    }

    #[test]
    fn test_bboxes_touching_overlap() {
        let a = LLBBox::new(0.0, 0.0, 10.0, 10.0).unwrap();
        let b = LLBBox::new(10.0, 0.0, 20.0, 10.0).unwrap();
        // Touching edges should overlap
        assert!(bboxes_overlap(&a, &b));
    }

    #[test]
    fn test_select_provider_global_default() {
        // Bbox outside all regional coverage gets the global provider
        let bbox = LLBBox::new(-33.86, 151.20, -33.85, 151.22).unwrap();
        let provider = select_provider(&bbox, SourceMode::Auto);
        assert_eq!(provider.name(), "mapterhorn");
    }

    #[test]
    fn test_select_provider_regional_beats_global() {
        // A US bbox keeps the 1m USGS 3DEP provider
        let bbox = LLBBox::new(40.0, -100.0, 40.01, -99.99).unwrap();
        let provider = select_provider(&bbox, SourceMode::Auto);
        assert_eq!(provider.name(), "usgs_3dep");
    }

    #[test]
    fn test_select_provider_global_only_skips_regional() {
        // Even bboxes inside regional coverage come back as Mapterhorn
        let bbox = LLBBox::new(40.0, -100.0, 40.01, -99.99).unwrap();
        let provider = select_provider(&bbox, SourceMode::GlobalOnly);
        assert_eq!(provider.name(), "mapterhorn");
    }

    #[test]
    fn test_select_provider_force_aws() {
        // Legacy mode always returns AWS regardless of coverage
        let bbox = LLBBox::new(40.0, -100.0, 40.01, -99.99).unwrap();
        let provider = select_provider(&bbox, SourceMode::AwsOnly);
        assert_eq!(provider.name(), "aws");
    }
}
