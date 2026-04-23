use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::ElevationProvider;
use crate::elevation::providers::aws_terrain::AwsTerrain;
use crate::elevation::providers::ign_france::IgnFrance;
use crate::elevation::providers::ign_spain::IgnSpain;
use crate::elevation::providers::regional::JapanGsi;
use crate::elevation::providers::usgs_3dep::Usgs3dep;

/// Check if two EPSG:4326 bounding boxes overlap.
pub fn bboxes_overlap(a: &LLBBox, b: &LLBBox) -> bool {
    a.min().lat() <= b.max().lat()
        && a.max().lat() >= b.min().lat()
        && a.min().lng() <= b.max().lng()
        && a.max().lng() >= b.min().lng()
}

/// Select the best elevation provider for the given bounding box.
///
/// Iterates providers ordered by resolution (finest first), returns the first
/// whose coverage overlaps the user's bbox. Falls back to AWS Terrain Tiles.
pub fn select_provider(bbox: &LLBBox) -> Box<dyn ElevationProvider> {
    let candidates: Vec<Box<dyn ElevationProvider>> = build_provider_list();

    for provider in candidates {
        if let Some(coverages) = provider.coverage_bboxes() {
            if coverages.iter().any(|c| bboxes_overlap(c, bbox)) {
                println!(
                    "Selected elevation provider: {} ({:.0}m resolution)",
                    provider.name(),
                    provider.native_resolution_m()
                );
                return provider;
            }
        }
    }

    // Global fallback
    println!("Using AWS Terrain Tiles (global fallback, ~30m resolution)");
    Box::new(AwsTerrain)
}

/// Build the list of available providers, ordered by resolution (finest first).
/// AWS Terrain Tiles is NOT included here -- it's the fallback.
fn build_provider_list() -> Vec<Box<dyn ElevationProvider>> {
    // Ordered by resolution (finest first). First match wins.
    // Only providers verified to return raw elevation data are enabled.
    vec![
        Box::new(Usgs3dep),  // 1.0m — ArcGIS REST, verified float32
        Box::new(IgnFrance), // 1.0m — WMS GeoTIFF, verified float32
        Box::new(IgnSpain),  // 5.0m — WCS, verified int16
        Box::new(JapanGsi),  // 5.0m — XYZ PNG tiles, custom encoding
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
    fn test_select_provider_fallback() {
        // Bbox outside all regional coverage should fall back to AWS
        let bbox = LLBBox::new(-33.86, 151.20, -33.85, 151.22).unwrap();
        let provider = select_provider(&bbox);
        assert_eq!(provider.name(), "aws");
    }
}
