use crate::coordinate_system::geographic::LLBBox;

/// Raw elevation grid in meters, before any Minecraft-specific processing.
/// NaN values indicate missing data that will be filled by post-processing.
pub struct RawElevationGrid {
    /// Height values in meters above sea level. NaN for missing data.
    pub heights_meters: Vec<Vec<f64>>,
}

/// Trait for elevation data providers.
///
/// Each provider handles its own CRS conversion, format decoding, and fetch protocol.
/// The contract: receive an EPSG:4326 bbox and grid dimensions, return elevation in meters.
pub trait ElevationProvider: Send + Sync {
    /// Human-readable name for logging and cache directory naming.
    fn name(&self) -> &'static str;

    /// Approximate native resolution in meters per pixel.
    fn native_resolution_m(&self) -> f64;

    /// Fetch raw elevation data for the given EPSG:4326 bbox,
    /// sampled onto a grid of the given dimensions.
    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>>;
}
