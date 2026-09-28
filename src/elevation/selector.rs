use crate::celestial::CelestialBody;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::provider::ElevationProvider;
use crate::elevation::providers::aws_terrain::AwsTerrain;
use crate::elevation::providers::mapterhorn::Mapterhorn;
use crate::elevation::providers::planetary::PlanetaryDem;

/// How the caller wants the elevation source chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMode {
    /// Mapterhorn, falling back to AWS. Used for generation and the 3D preview.
    Auto,
    /// Legacy AWS tiles only (--aws-only-elevation / "Legacy terrain" toggle).
    AwsOnly,
    /// NASA PDS raster for a non-Earth body. No fallback: an Earth provider would
    /// return sea-level noise for these coordinates.
    Planetary(CelestialBody),
}

impl SourceMode {
    /// Whether the Earth fallback chain may be appended after the selection.
    pub fn allows_earth_fallback(self) -> bool {
        !matches!(self, SourceMode::Planetary(_))
    }
}

/// Select the elevation provider for the given bounding box.
/// The caller chains the fetch-time fallback to AWS.
///
/// Mapterhorn serves Earth everywhere: its pyramid carries the national LiDAR
/// and DEM surveys (USGS 3DEP 1 m, Canada's HRDEM, most of Europe, Japan, New
/// Zealand, ...) above a global 30 m floor, and falls back per tile, so one
/// source covers any bbox without seams between services.
pub fn select_provider(_bbox: &LLBBox, mode: SourceMode) -> Box<dyn ElevationProvider> {
    match mode {
        SourceMode::AwsOnly => {
            println!("Using AWS Terrain Tiles only (legacy mode, ~30m resolution)");
            Box::new(AwsTerrain)
        }
        SourceMode::Planetary(body) => Box::new(PlanetaryDem { body }),
        SourceMode::Auto => {
            println!("Using Mapterhorn terrain tiles (global; high-res where available)");
            Box::new(Mapterhorn)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_provider_global_default() {
        let bbox = LLBBox::new(-33.86, 151.20, -33.85, 151.22).unwrap();
        let provider = select_provider(&bbox, SourceMode::Auto);
        assert_eq!(provider.name(), "mapterhorn");
    }

    #[test]
    fn test_select_provider_uses_mapterhorn_in_north_america() {
        // Mapterhorn carries 3DEP 1 m in the US and HRDEM in Canada, so neither
        // needs a regional service, and a Canadian city is never sent to a US one.
        for bbox in [
            LLBBox::new(40.0, -100.0, 40.01, -99.99).unwrap(),
            LLBBox::new(49.215467, -123.266945, 49.280852, -123.177338).unwrap(),
        ] {
            assert_eq!(
                select_provider(&bbox, SourceMode::Auto).name(),
                "mapterhorn"
            );
        }
    }

    #[test]
    fn test_select_provider_force_aws() {
        // Legacy mode always returns AWS regardless of coverage
        let bbox = LLBBox::new(40.0, -100.0, 40.01, -99.99).unwrap();
        let provider = select_provider(&bbox, SourceMode::AwsOnly);
        assert_eq!(provider.name(), "aws");
    }
}
