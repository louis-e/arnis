use crate::coordinate_system::geographic::LLBBox;

/// Raw elevation grid in meters, before any Minecraft-specific processing.
/// NaN values indicate missing data that will be filled by post-processing.
pub struct RawElevationGrid {
    /// Height values in meters above sea level. NaN for missing data.
    pub heights_meters: Vec<Vec<f64>>,
    /// The source resolution behind the heights, where the provider knows it per place.
    pub source_pixels: Option<SourcePixels>,
}

/// The source pixel size in metres behind every grid cell, as runs along each row: what
/// the provider really had there, which the resampled heights no longer show. A cell
/// with no data reads infinity. Coverage changes only at tile edges, so a row holds a
/// handful of runs whatever the grid's size.
pub struct SourcePixels {
    rows: Vec<SourceRow>,
}

/// One grid row of `SourcePixels`: (first column, metres) per run, in column order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceRow(Vec<(u32, f32)>);

impl SourceRow {
    /// Notes the source pixel behind column `gx`, the column after the last one noted.
    #[inline]
    pub fn push(&mut self, gx: usize, metres: f32) {
        if self.0.last().is_none_or(|&(_, m)| m != metres) {
            self.0.push((gx as u32, metres));
        }
    }

    /// The runs as (first column, metres).
    pub fn runs(&self) -> &[(u32, f32)] {
        &self.0
    }
}

impl SourcePixels {
    pub fn new(rows: Vec<SourceRow>) -> Self {
        Self { rows }
    }

    /// Grid rows covered.
    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// Runs of grid row `gz`.
    pub fn row(&self, gz: usize) -> &SourceRow {
        &self.rows[gz]
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_row_keeps_one_run_per_stretch() {
        let mut row = SourceRow::default();
        for (gx, m) in [0.8, 0.8, 0.8, 30.0, 30.0, f32::INFINITY, 0.8]
            .into_iter()
            .enumerate()
        {
            row.push(gx, m);
        }
        assert_eq!(
            row.runs(),
            &[(0, 0.8), (3, 30.0), (5, f32::INFINITY), (6, 0.8)]
        );
    }
}
