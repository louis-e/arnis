//! Urban ground detection and generation based on building clusters.
//!
//! This module computes urban areas by analyzing building density and clustering,
//! then generates appropriate ground blocks (smooth stone) for those areas.
//!
//! # Algorithm Overview
//!
//! 1. **Grid-based density analysis**: Divide the world into cells and count buildings per cell
//! 2. **Connected component detection**: Find clusters of dense cells using flood fill
//! 3. **Cluster filtering**: Only keep clusters with enough buildings to be considered "urban"
//! 4. **Concave hull computation**: Compute a tight-fitting boundary around each cluster
//! 5. **Ground filling**: Fill the hull area with stone blocks
//!
//! This approach handles various scenarios:
//! - Full city coverage: Large connected cluster
//! - Multiple cities: Separate clusters, each gets its own hull
//! - Rural areas: No clusters meet threshold, no stone placed
//! - Isolated buildings: Don't meet cluster threshold, remain on grass

use crate::coordinate_system::cartesian::XZBBox;
use crate::floodfill::flood_fill_area;
use geo::{ConcaveHull, ConvexHull, MultiPoint, Point, Polygon, Simplify};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

/// Configuration for urban ground detection.
///
/// These parameters control how building clusters are identified and
/// how the urban ground boundary is computed.
#[derive(Debug, Clone)]
pub struct UrbanGroundConfig {
    /// Grid cell size for density analysis (in blocks).
    /// Smaller = more precise but slower. Default: 64 blocks (4 chunks).
    pub cell_size: i32,

    /// Minimum buildings per cell to consider it potentially urban.
    /// Cells below this threshold are ignored. Default: 1.
    pub min_buildings_per_cell: usize,

    /// Minimum total buildings in a connected cluster to be considered urban.
    /// Small clusters (villages, isolated buildings) won't get stone ground. Default: 5.
    pub min_buildings_for_cluster: usize,

    /// Concavity parameter for hull computation (used in legacy hull-based method).
    /// Lower = tighter fit to buildings (more concave), Higher = smoother (more convex).
    /// Range: 1.0 (very tight) to 10.0 (almost convex). Default: 2.0.
    pub concavity: f64,

    /// Whether to expand the hull slightly beyond building boundaries (used in legacy method).
    /// This creates a small buffer zone around the urban area. Default: true.
    pub expand_hull: bool,

    /// Base number of cells to expand the urban region.
    /// This helps fill small gaps between buildings. Adaptive expansion may increase this.
    /// Default: 2.
    pub cell_expansion: i32,
}

impl Default for UrbanGroundConfig {
    fn default() -> Self {
        Self {
            cell_size: 64, // Smaller cells for better granularity (4 chunks instead of 6)
            min_buildings_per_cell: 1,
            min_buildings_for_cluster: 5,
            concavity: 2.0,
            expand_hull: true,
            cell_expansion: 2, // Larger expansion to connect spread-out buildings
        }
    }
}

/// Represents a detected urban cluster with its buildings and computed boundary.
#[derive(Debug)]
#[allow(dead_code)]
pub struct UrbanCluster {
    /// Grid cells that belong to this cluster
    cells: Vec<(i32, i32)>,
    /// Building centroids within this cluster
    building_centroids: Vec<(i32, i32)>,
    /// Total number of buildings in the cluster
    building_count: usize,
}

/// Computes urban ground areas from building locations.
pub struct UrbanGroundComputer {
    config: UrbanGroundConfig,
    building_centroids: Vec<(i32, i32)>,
    xzbbox: XZBBox,
}

impl UrbanGroundComputer {
    /// Creates a new urban ground computer with the given world bounds and configuration.
    pub fn new(xzbbox: XZBBox, config: UrbanGroundConfig) -> Self {
        Self {
            config,
            building_centroids: Vec::new(),
            xzbbox,
        }
    }

    /// Creates a new urban ground computer with default configuration.
    pub fn with_defaults(xzbbox: XZBBox) -> Self {
        Self::new(xzbbox, UrbanGroundConfig::default())
    }

    /// Adds a building centroid to be considered for urban area detection.
    #[inline]
    pub fn add_building_centroid(&mut self, x: i32, z: i32) {
        // Only add if within bounds
        if x >= self.xzbbox.min_x()
            && x <= self.xzbbox.max_x()
            && z >= self.xzbbox.min_z()
            && z <= self.xzbbox.max_z()
        {
            self.building_centroids.push((x, z));
        }
    }

    /// Adds multiple building centroids from an iterator.
    pub fn add_building_centroids<I>(&mut self, centroids: I)
    where
        I: IntoIterator<Item = (i32, i32)>,
    {
        for (x, z) in centroids {
            self.add_building_centroid(x, z);
        }
    }

    /// Returns the number of buildings added.
    #[allow(dead_code)]
    pub fn building_count(&self) -> usize {
        self.building_centroids.len()
    }

    /// Computes all urban ground coordinates.
    ///
    /// Returns a list of (x, z) coordinates that should have stone ground.
    /// The coordinates are clipped to the world bounding box.
    ///
    /// Performance: Uses cell-based filling for O(cells) complexity instead of
    /// flood-filling complex hulls which would be O(area). For a city with 1000
    /// buildings in 100 cells, this is ~100x faster than flood fill.
    pub fn compute(&self, _timeout: Option<&Duration>) -> Vec<(i32, i32)> {
        // Not enough buildings for any urban area
        if self.building_centroids.len() < self.config.min_buildings_for_cluster {
            return Vec::new();
        }

        // Step 1: Create density grid (cell -> buildings in that cell)
        let grid = self.create_density_grid();

        // Step 2: Find connected urban regions and get their expanded cells
        let clusters = self.find_urban_clusters(&grid);

        if clusters.is_empty() {
            return Vec::new();
        }

        // Step 3: Fill cells directly instead of using expensive flood fill on hulls
        // This is much faster: O(cells × cell_size²) vs O(hull_area) for flood fill
        let mut all_coords = Vec::new();
        for cluster in clusters {
            let coords = self.fill_cluster_cells(&cluster);
            all_coords.extend(coords);
        }

        all_coords
    }

    /// Fills all cells in a cluster directly, returning coordinates.
    /// This is much faster than computing a hull and flood-filling it.
    fn fill_cluster_cells(&self, cluster: &UrbanCluster) -> Vec<(i32, i32)> {
        let mut coords = Vec::new();
        let cell_size = self.config.cell_size;

        // Pre-calculate bounds once
        let bbox_min_x = self.xzbbox.min_x();
        let bbox_max_x = self.xzbbox.max_x();
        let bbox_min_z = self.xzbbox.min_z();
        let bbox_max_z = self.xzbbox.max_z();

        for &(cx, cz) in &cluster.cells {
            // Calculate cell bounds in world coordinates
            let cell_min_x = (bbox_min_x + cx * cell_size).max(bbox_min_x);
            let cell_max_x = (bbox_min_x + (cx + 1) * cell_size - 1).min(bbox_max_x);
            let cell_min_z = (bbox_min_z + cz * cell_size).max(bbox_min_z);
            let cell_max_z = (bbox_min_z + (cz + 1) * cell_size - 1).min(bbox_max_z);

            // Skip if cell is entirely outside bbox
            if cell_min_x > bbox_max_x
                || cell_max_x < bbox_min_x
                || cell_min_z > bbox_max_z
                || cell_max_z < bbox_min_z
            {
                continue;
            }

            // Fill all coordinates in this cell
            for x in cell_min_x..=cell_max_x {
                for z in cell_min_z..=cell_max_z {
                    coords.push((x, z));
                }
            }
        }

        coords
    }

    /// Creates a density grid mapping cell coordinates to buildings in that cell.
    fn create_density_grid(&self) -> HashMap<(i32, i32), Vec<(i32, i32)>> {
        let mut grid: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();

        for &(x, z) in &self.building_centroids {
            let cell_x = (x - self.xzbbox.min_x()) / self.config.cell_size;
            let cell_z = (z - self.xzbbox.min_z()) / self.config.cell_size;
            grid.entry((cell_x, cell_z)).or_default().push((x, z));
        }

        grid
    }

    /// Finds connected clusters of urban cells.
    fn find_urban_clusters(
        &self,
        grid: &HashMap<(i32, i32), Vec<(i32, i32)>>,
    ) -> Vec<UrbanCluster> {
        // Step 1: Identify cells that meet minimum density threshold
        let dense_cells: HashSet<(i32, i32)> = grid
            .iter()
            .filter(|(_, buildings)| buildings.len() >= self.config.min_buildings_per_cell)
            .map(|(&cell, _)| cell)
            .collect();

        if dense_cells.is_empty() {
            return Vec::new();
        }

        // Step 2: Calculate adaptive expansion based on building density
        // For spread-out cities, we need more expansion to connect buildings
        let adaptive_expansion = self.calculate_adaptive_expansion(&dense_cells, grid);

        // Step 3: Expand dense cells to connect nearby clusters
        let expanded_cells = self.expand_cells_adaptive(&dense_cells, adaptive_expansion);

        // Step 4: Find connected components using flood fill
        let mut visited = HashSet::new();
        let mut clusters = Vec::new();

        for &cell in &expanded_cells {
            if visited.contains(&cell) {
                continue;
            }

            // BFS to find connected component
            let mut component_cells = Vec::new();
            let mut queue = VecDeque::new();
            queue.push_back(cell);
            visited.insert(cell);

            while let Some(current) = queue.pop_front() {
                component_cells.push(current);

                // Check 8-connected neighbors (including diagonals for better connectivity)
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dz == 0 {
                            continue;
                        }
                        let neighbor = (current.0 + dx, current.1 + dz);
                        if expanded_cells.contains(&neighbor) && !visited.contains(&neighbor) {
                            visited.insert(neighbor);
                            queue.push_back(neighbor);
                        }
                    }
                }
            }

            // Collect buildings from the original dense cells only (not expanded empty cells)
            let mut cluster_buildings = Vec::new();
            for &cell in &component_cells {
                if let Some(buildings) = grid.get(&cell) {
                    cluster_buildings.extend(buildings.iter().copied());
                }
            }

            let building_count = cluster_buildings.len();

            // Only keep clusters with enough buildings
            if building_count >= self.config.min_buildings_for_cluster {
                clusters.push(UrbanCluster {
                    cells: component_cells,
                    building_centroids: cluster_buildings,
                    building_count,
                });
            }
        }

        clusters
    }

    /// Calculates adaptive expansion based on building density.
    ///
    /// For spread-out cities (low density), we need more expansion to connect
    /// buildings that are farther apart. For dense cities, less expansion is needed.
    fn calculate_adaptive_expansion(
        &self,
        dense_cells: &HashSet<(i32, i32)>,
        grid: &HashMap<(i32, i32), Vec<(i32, i32)>>,
    ) -> i32 {
        if dense_cells.is_empty() {
            return self.config.cell_expansion;
        }

        // Calculate total buildings and average per occupied cell
        let total_buildings: usize = dense_cells
            .iter()
            .filter_map(|cell| grid.get(cell))
            .map(|buildings| buildings.len())
            .sum();

        let avg_buildings_per_cell = total_buildings as f64 / dense_cells.len() as f64;

        // Calculate the "spread" of cells - how far apart are occupied cells?
        // Find bounding box of occupied cells
        if dense_cells.len() < 2 {
            return self.config.cell_expansion;
        }

        let min_x = dense_cells.iter().map(|(x, _)| x).min().unwrap();
        let max_x = dense_cells.iter().map(|(x, _)| x).max().unwrap();
        let min_z = dense_cells.iter().map(|(_, z)| z).min().unwrap();
        let max_z = dense_cells.iter().map(|(_, z)| z).max().unwrap();

        let grid_span_x = (max_x - min_x + 1) as f64;
        let grid_span_z = (max_z - min_z + 1) as f64;
        let total_possible_cells = grid_span_x * grid_span_z;

        // Cell occupancy ratio: what fraction of the bounding box has buildings?
        let occupancy = dense_cells.len() as f64 / total_possible_cells;

        // Adaptive expansion logic:
        // - High density (many buildings per cell) AND high occupancy = dense city, use base expansion
        // - Low density OR low occupancy = spread-out city, need more expansion

        let base_expansion = self.config.cell_expansion;

        // Scale factor: lower density = higher factor
        // avg_buildings_per_cell < 2 → spread out
        // occupancy < 0.3 → sparse grid with gaps
        let density_factor = if avg_buildings_per_cell < 3.0 {
            1.5
        } else {
            1.0
        };
        let occupancy_factor = if occupancy < 0.4 {
            1.5
        } else if occupancy < 0.6 {
            1.25
        } else {
            1.0
        };

        let adaptive = (base_expansion as f64 * density_factor * occupancy_factor).ceil() as i32;

        // Cap at reasonable maximum (4 cells = 256 blocks with 64-block cells)
        adaptive.min(4).max(base_expansion)
    }

    /// Expands the set of cells by adding neighbors within expansion distance.
    fn expand_cells_adaptive(
        &self,
        cells: &HashSet<(i32, i32)>,
        expansion: i32,
    ) -> HashSet<(i32, i32)> {
        if expansion <= 0 {
            return cells.clone();
        }

        let mut expanded = cells.clone();

        for &(cx, cz) in cells {
            for dz in -expansion..=expansion {
                for dx in -expansion..=expansion {
                    expanded.insert((cx + dx, cz + dz));
                }
            }
        }

        expanded
    }

    /// Expands the set of cells by adding neighbors within expansion distance.
    #[allow(dead_code)]
    fn expand_cells(&self, cells: &HashSet<(i32, i32)>) -> HashSet<(i32, i32)> {
        self.expand_cells_adaptive(cells, self.config.cell_expansion)
    }

    /// Computes ground coordinates for a single urban cluster.
    ///
    /// NOTE: This hull-based method is kept for reference but not used in production.
    /// The cell-based `fill_cluster_cells` method is much faster.
    #[allow(dead_code)]
    fn compute_cluster_ground(
        &self,
        cluster: &UrbanCluster,
        grid: &HashMap<(i32, i32), Vec<(i32, i32)>>,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        // Need at least 3 points for a hull
        if cluster.building_centroids.len() < 3 {
            return Vec::new();
        }

        // Collect points for hull computation
        // Include building centroids plus cell corner points for better coverage
        let mut hull_points: Vec<(f64, f64)> = cluster
            .building_centroids
            .iter()
            .map(|&(x, z)| (x as f64, z as f64))
            .collect();

        // Add cell boundary points if expand_hull is enabled
        // This ensures the hull extends slightly beyond buildings
        if self.config.expand_hull {
            for &(cx, cz) in &cluster.cells {
                // Only add corners for cells that actually have buildings
                if grid.get(&(cx, cz)).map(|b| !b.is_empty()).unwrap_or(false) {
                    let base_x = (self.xzbbox.min_x() + cx * self.config.cell_size) as f64;
                    let base_z = (self.xzbbox.min_z() + cz * self.config.cell_size) as f64;
                    let size = self.config.cell_size as f64;

                    // Add cell corners with small padding
                    let pad = size * 0.1; // 10% padding
                    hull_points.push((base_x - pad, base_z - pad));
                    hull_points.push((base_x + size + pad, base_z - pad));
                    hull_points.push((base_x - pad, base_z + size + pad));
                    hull_points.push((base_x + size + pad, base_z + size + pad));
                }
            }
        }

        // Convert to geo MultiPoint
        let multi_point: MultiPoint<f64> =
            hull_points.iter().map(|&(x, z)| Point::new(x, z)).collect();

        // Compute hull based on point count
        let hull: Polygon<f64> = if hull_points.len() < 10 {
            // Too few points for concave hull, use convex
            multi_point.convex_hull()
        } else {
            // Use concave hull for better fit
            multi_point.concave_hull(self.config.concavity)
        };

        // Simplify the hull to reduce vertex count (improves flood fill performance)
        let hull = hull.simplify(2.0);

        // Convert hull to integer coordinates for flood fill
        self.fill_hull_polygon(&hull, timeout)
    }

    /// Fills a hull polygon and returns all interior coordinates.
    ///
    /// NOTE: This method is kept for reference but not used in production.
    /// The cell-based approach is much faster.
    #[allow(dead_code)]
    fn fill_hull_polygon(
        &self,
        polygon: &Polygon<f64>,
        timeout: Option<&Duration>,
    ) -> Vec<(i32, i32)> {
        // Convert polygon exterior to integer coordinates
        let exterior: Vec<(i32, i32)> = polygon
            .exterior()
            .coords()
            .map(|c| (c.x.round() as i32, c.y.round() as i32))
            .collect();

        if exterior.len() < 3 {
            return Vec::new();
        }

        // Remove duplicate consecutive points (can cause flood fill issues)
        let mut clean_exterior = Vec::with_capacity(exterior.len());
        for point in exterior {
            if clean_exterior.last() != Some(&point) {
                clean_exterior.push(point);
            }
        }

        // Ensure the polygon is closed
        if clean_exterior.first() != clean_exterior.last() && !clean_exterior.is_empty() {
            clean_exterior.push(clean_exterior[0]);
        }

        if clean_exterior.len() < 4 {
            // Need at least 3 unique points + closing point
            return Vec::new();
        }

        // Use existing flood fill, clipping to bbox
        let filled = flood_fill_area(&clean_exterior, timeout);

        // Filter to only include points within world bounds
        filled
            .into_iter()
            .filter(|&(x, z)| {
                x >= self.xzbbox.min_x()
                    && x <= self.xzbbox.max_x()
                    && z >= self.xzbbox.min_z()
                    && z <= self.xzbbox.max_z()
            })
            .collect()
    }
}

/// Computes the centroid of a set of coordinates.
///
/// Returns None if the slice is empty.
#[inline]
#[allow(dead_code)]
pub fn compute_centroid(coords: &[(i32, i32)]) -> Option<(i32, i32)> {
    if coords.is_empty() {
        return None;
    }
    let sum_x: i64 = coords.iter().map(|(x, _)| i64::from(*x)).sum();
    let sum_z: i64 = coords.iter().map(|(_, z)| i64::from(*z)).sum();
    let len = coords.len() as i64;
    Some(((sum_x / len) as i32, (sum_z / len) as i32))
}

/// Convenience function to compute urban ground from building centroids.
///
/// This is the main entry point for urban ground generation.
pub fn compute_urban_ground(
    building_centroids: Vec<(i32, i32)>,
    xzbbox: &XZBBox,
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    let mut computer = UrbanGroundComputer::with_defaults(xzbbox.clone());
    computer.add_building_centroids(building_centroids);
    computer.compute(timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_bbox() -> XZBBox {
        XZBBox::rect_from_xz_lengths(1000.0, 1000.0).unwrap()
    }

    #[test]
    fn test_no_buildings() {
        let computer = UrbanGroundComputer::with_defaults(create_test_bbox());
        let result = computer.compute(None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_few_scattered_buildings() {
        let mut computer = UrbanGroundComputer::with_defaults(create_test_bbox());
        // Add a few scattered buildings (not enough for a cluster)
        computer.add_building_centroid(100, 100);
        computer.add_building_centroid(500, 500);
        computer.add_building_centroid(900, 900);

        let result = computer.compute(None);
        assert!(
            result.is_empty(),
            "Scattered buildings should not form urban area"
        );
    }

    #[test]
    fn test_dense_cluster() {
        let mut computer = UrbanGroundComputer::with_defaults(create_test_bbox());

        // Add a dense cluster of buildings
        for i in 0..30 {
            for j in 0..30 {
                if (i + j) % 3 == 0 {
                    // Add building every 3rd position
                    computer.add_building_centroid(100 + i * 10, 100 + j * 10);
                }
            }
        }

        let result = computer.compute(None);
        assert!(
            !result.is_empty(),
            "Dense cluster should produce urban area"
        );
    }

    #[test]
    fn test_compute_centroid() {
        let coords = vec![(0, 0), (10, 0), (10, 10), (0, 10)];
        let centroid = compute_centroid(&coords);
        assert_eq!(centroid, Some((5, 5)));
    }

    #[test]
    fn test_compute_centroid_empty() {
        let coords: Vec<(i32, i32)> = vec![];
        let centroid = compute_centroid(&coords);
        assert_eq!(centroid, None);
    }

    #[test]
    fn test_spread_out_buildings() {
        // Simulate a spread-out city like Erding where buildings are farther apart
        // This should still be detected as urban due to adaptive expansion
        let mut computer = UrbanGroundComputer::with_defaults(create_test_bbox());

        // Add buildings spread across a larger area with gaps
        // Buildings are ~100-150 blocks apart (would fail with small expansion)
        let building_positions = [
            (100, 100),
            (250, 100),
            (400, 100),
            (100, 250),
            (250, 250),
            (400, 250),
            (100, 400),
            (250, 400),
            (400, 400),
            // Add a few more to ensure cluster threshold is met
            (175, 175),
            (325, 175),
            (175, 325),
            (325, 325),
        ];

        for (x, z) in building_positions {
            computer.add_building_centroid(x, z);
        }

        let result = computer.compute(None);
        assert!(
            !result.is_empty(),
            "Spread-out buildings should still form urban area with adaptive expansion"
        );
    }

    #[test]
    fn test_adaptive_expansion_calculated() {
        let bbox = create_test_bbox();
        let computer = UrbanGroundComputer::with_defaults(bbox);

        // Create a sparse grid with low occupancy
        let mut dense_cells = HashSet::new();
        // Only 4 cells in a 10x10 potential grid = 4% occupancy
        dense_cells.insert((0, 0));
        dense_cells.insert((5, 0));
        dense_cells.insert((0, 5));
        dense_cells.insert((5, 5));

        let mut grid = HashMap::new();
        // Only 1 building per cell (low density)
        grid.insert((0, 0), vec![(10, 10)]);
        grid.insert((5, 0), vec![(330, 10)]);
        grid.insert((0, 5), vec![(10, 330)]);
        grid.insert((5, 5), vec![(330, 330)]);

        let expansion = computer.calculate_adaptive_expansion(&dense_cells, &grid);

        // Should be higher than base (2) due to low occupancy and density
        assert!(
            expansion > 2,
            "Sparse grid should trigger higher expansion, got {}",
            expansion
        );
    }
}
