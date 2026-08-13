//! Coarse road-bearing grid: which way do the roads run, per area of the map.
//!
//! Field parcels align to their access roads in real agricultural land. This module
//! aggregates the direction of OSM highway segments into a coarse grid (doubled-angle
//! sums, so a road and its reverse count as the same bearing). The field-texture
//! orientation domains then take their rotation from the dominant nearby road instead
//! of from a hash, and fall back to the hashed angle where there are no roads.
//!
//! Seam-critical: the grid is anchored to a world lattice (`div_euclid(CELL)` on
//! absolute coordinates), never to a tile's own bbox, so a point lands in the same
//! lattice cell no matter which tile resolves it. Segments outside a tile's bbox keep
//! their true lattice cell rather than being clamped into the edge cells, since
//! clamping would pollute border cells with the bearing of far-away roads.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::osm_parser::ProcessedElement;

/// Grid cell edge (blocks) on the world lattice.
///
/// Seam-critical size: a domain asks for the bearing at its own centre, and that centre
/// can sit up to MACRO/2 (96) blocks outside the area a tile keeps. A tile only knows
/// the OSM inside its own bbox, so the bearing may only depend on road data within a
/// short radius of the query point. One 64-block cell (radius 32) keeps the total reach
/// at 128 blocks. Summing over a 3x3 neighbourhood instead (radius 192) lets two
/// neighbouring tiles clip different road sets into the same query and resolve different
/// rotations, which rotates the whole parcel grid at the seam.
const CELL: i32 = 64;
/// Minimum accumulated road length (blocks) in the queried cell before the roads are
/// considered to define an orientation.
const MIN_MAGNITUDE: f64 = 18.0;
/// Bearings snap to this step (radians, 7.5 degrees). Two tiles can see marginally
/// different road sets near their shared border; quantising makes both land on the same
/// angle unless the difference is genuinely large.
const ANGLE_STEP: f64 = std::f64::consts::PI / 24.0;

/// Doubled-angle direction sums per world lattice cell, folding theta and theta+180
/// onto the same bearing.
type BearingGrid = HashMap<(i32, i32), (f64, f64)>;

static GRID: RwLock<Option<BearingGrid>> = RwLock::new(None);

/// Build the bearing grid from this run's OSM elements. Called once per generation,
/// before element and ground processing.
pub fn set_from_elements(elements: &[ProcessedElement]) {
    let mut grid: BearingGrid = HashMap::new();
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        let Some(hw) = way.tags.get("highway") else {
            continue;
        };
        // Skip point-like and footway noise; keep everything that reads as a road or lane.
        if matches!(
            hw.as_str(),
            "street_lamp" | "crossing" | "bus_stop" | "steps"
        ) {
            continue;
        }
        for pair in way.nodes.windows(2) {
            let (x1, z1) = (pair[0].x, pair[0].z);
            let (x2, z2) = (pair[1].x, pair[1].z);
            let dx = (x2 - x1) as f64;
            let dz = (z2 - z1) as f64;
            let len = (dx * dx + dz * dz).sqrt();
            if len < 1.0 {
                continue;
            }
            let theta = dz.atan2(dx);
            let (c2, s2) = ((2.0 * theta).cos() * len, (2.0 * theta).sin() * len);
            // Stamp the segment's bearing into every lattice cell it passes through.
            let steps = (len / CELL as f64).ceil() as i32 + 1;
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                let px = (x1 as f64 + dx * t) as i32;
                let pz = (z1 as f64 + dz * t) as i32;
                let e = grid
                    .entry((px.div_euclid(CELL), pz.div_euclid(CELL)))
                    .or_insert((0.0, 0.0));
                e.0 += c2;
                e.1 += s2;
            }
        }
    }
    *GRID.write().unwrap() = Some(grid);
}

/// Dominant road bearing near world point (x, z), as (sin, cos) of the grid angle,
/// ready to rotate parcel coordinates. None when no road is near enough to matter.
pub fn bearing_at(x: i32, z: i32) -> Option<(f64, f64)> {
    let guard = GRID.read().unwrap();
    let grid = guard.as_ref()?;
    // One cell only (radius 32 blocks). Widening this re-introduces the seam: see the
    // CELL comment. The query point can already be 96 blocks outside the tile's kept
    // area, and every extra block of radius eats into the overlap both neighbours share.
    let (sx, sz) = match grid.get(&(x.div_euclid(CELL), z.div_euclid(CELL))) {
        Some(&(sx, sz)) => (sx, sz),
        None => return None,
    };
    // Need a meaningful amount of road, in blocks of accumulated length.
    if (sx * sx + sz * sz).sqrt() < MIN_MAGNITUDE {
        return None;
    }
    // Quantise: identical for both tiles unless the road sets differ substantially.
    let theta = (0.5 * sz.atan2(sx) / ANGLE_STEP).round() * ANGLE_STEP;
    Some((theta.sin(), theta.cos()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lattice must be world-anchored: a point's grid cell is a pure function of its
    /// absolute coordinates, with no dependence on any tile origin.
    #[test]
    fn lattice_is_world_anchored() {
        let cases: [(i32, i32); 6] = [
            (0, 0),
            (CELL - 1, 0),
            (CELL, 1),
            (-1, -1),
            (-CELL, -1),
            (-CELL - 1, -2),
        ];
        for (x, gx) in cases {
            assert_eq!(x.div_euclid(CELL), gx, "x={x}");
        }
    }

    /// The seam property: the bearing at a point depends only on the road data inside
    /// that point's own lattice cell. Two tiles that clipped different road sets outside
    /// it, even in the immediately adjacent cell, must still resolve the same bearing,
    /// because every extra block of reach is data one of the two tiles may not have.
    #[test]
    fn bearing_depends_only_on_its_own_cell() {
        let own = (100.0, 0.0);
        let mut grid: BearingGrid = HashMap::new();
        grid.insert((0, 0), own);
        grid.insert((1, 0), (-500.0, 250.0)); // adjacent cell, only one tile sees it
        grid.insert((0, -1), (-500.0, -250.0));
        grid.insert((9, 9), (-100.0, 50.0)); // far away
        *GRID.write().unwrap() = Some(grid);
        let a = bearing_at(10, 10);

        let mut grid2: BearingGrid = HashMap::new();
        grid2.insert((0, 0), own); // the other tile saw only the own-cell road
        *GRID.write().unwrap() = Some(grid2);
        let b = bearing_at(10, 10);

        assert_eq!(
            a.is_some(),
            b.is_some(),
            "one tile found a road, the other not"
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert!(
            (a.0 - b.0).abs() < 1e-12 && (a.1 - b.1).abs() < 1e-12,
            "neighbouring-cell roads changed the bearing: {a:?} vs {b:?}"
        );

        // A cell with no road of its own yields no bearing. The parcel grid then falls
        // back to its hashed angle, which is a pure function of position.
        let mut grid3: BearingGrid = HashMap::new();
        grid3.insert((1, 0), (500.0, 0.0));
        *GRID.write().unwrap() = Some(grid3);
        assert!(bearing_at(10, 10).is_none());
        *GRID.write().unwrap() = None;
    }
}
