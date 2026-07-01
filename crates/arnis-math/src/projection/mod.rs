pub mod web_mercator;

pub use web_mercator::WebMercatorProjection;

use std::fmt;
use std::str::FromStr;

/// Trait for converting between WGS84 geographic coordinates and a projected
/// coordinate system used in Minecraft world generation.
pub trait Projection {
    /// Convert WGS84 latitude/longitude (degrees) to projected (x, z) in meters
    /// (or blocks, depending on scale).
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64);

    /// Convert projected (x, z) back to WGS84 latitude/longitude (degrees).
    /// Defined for completeness of the projection interface; current generation
    /// flow only needs `forward`, but reverse-projection is needed if we ever
    /// surface real-world coordinates back from a Minecraft point.
    #[allow(dead_code)]
    fn inverse(&self, x: f64, z: f64) -> (f64, f64);
}

/// Available map projection variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionKind {
    /// Web Mercator (EPSG:3857-like) projection with a local origin offset.
    WebMercator,
    /// Simple local coordinate system (no geographic projection).
    Local,
}

impl fmt::Display for ProjectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectionKind::WebMercator => write!(f, "web_mercator"),
            ProjectionKind::Local => write!(f, "local"),
        }
    }
}

impl FromStr for ProjectionKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "web_mercator" | "webmercator" | "mercator" => Ok(ProjectionKind::WebMercator),
            "local" => Ok(ProjectionKind::Local),
            other => Err(format!("unknown projection kind: '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projection_kind_display() {
        assert_eq!(ProjectionKind::WebMercator.to_string(), "web_mercator");
        assert_eq!(ProjectionKind::Local.to_string(), "local");
    }

    #[test]
    fn test_projection_kind_from_str() {
        assert_eq!(
            "web_mercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "webmercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "mercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "local".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::Local
        );
        assert_eq!(
            "LOCAL".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::Local
        );
    }

    #[test]
    fn test_projection_kind_from_str_invalid() {
        assert!("unknown".parse::<ProjectionKind>().is_err());
    }

    #[test]
    fn test_projection_kind_roundtrip() {
        for kind in [ProjectionKind::WebMercator, ProjectionKind::Local] {
            let s = kind.to_string();
            let parsed: ProjectionKind = s.parse().unwrap();
            assert_eq!(parsed, kind);
        }
    }
}
