//! Coarse lowland grid: which parts of the map sit low and flat.
//!
//! Sunflower fields cluster in low, open plains in real agriculture. This samples the
//! terrain height on a coarse lattice and marks the low band, letting the farm-crop
//! picker boost sunflower plots there.
//!
//! Seam-critical: the lattice is anchored to the world (`div_euclid(CELL)` on absolute
//! coordinates), never to a tile's bbox, so a point lands in the same lattice cell no
//! matter which tile resolves it.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;

const CELL: i32 = 128;
/// Lowland is the bottom this-much of the sampled terrain, by rank.
const LOW_PERCENTILE: f64 = 0.35;

struct LowlandGrid {
    /// World lattice cell -> low?
    low: HashMap<(i32, i32), bool>,
}

static GRID: RwLock<Option<LowlandGrid>> = RwLock::new(None);

/// Build the lowland grid from the run's terrain. Called once per generation.
pub fn set_from_ground(ground: &Ground, xzbbox: &XZBBox) {
    let (min_x, min_z) = (xzbbox.min_x(), xzbbox.min_z());
    let (max_x, max_z) = (xzbbox.max_x(), xzbbox.max_z());
    // Every world lattice cell that overlaps this area, sampled at its true lattice
    // centre, clamped inward only near the data edge.
    let (gx0, gx1) = (min_x.div_euclid(CELL), max_x.div_euclid(CELL));
    let (gz0, gz1) = (min_z.div_euclid(CELL), max_z.div_euclid(CELL));
    let mut samples: Vec<((i32, i32), i32)> = Vec::new();
    for gz in gz0..=gz1 {
        for gx in gx0..=gx1 {
            let cx = (gx * CELL + CELL / 2).clamp(min_x, max_x);
            let cz = (gz * CELL + CELL / 2).clamp(min_z, max_z);
            samples.push(((gx, gz), ground.level(XZPoint::new(cx, cz))));
        }
    }
    if samples.is_empty() {
        *GRID.write().unwrap() = None;
        return;
    }
    let mut sorted: Vec<i32> = samples.iter().map(|&(_, y)| y).collect();
    sorted.sort_unstable();
    let threshold = sorted[(sorted.len() as f64 * LOW_PERCENTILE) as usize % sorted.len()];
    let low = samples
        .into_iter()
        .map(|(key, y)| (key, y <= threshold))
        .collect();
    *GRID.write().unwrap() = Some(LowlandGrid { low });
}

/// True when world point (x, z) sits in the low band of the terrain. Lattice cells the
/// grid never saw read as not-low.
pub fn is_lowland(x: i32, z: i32) -> bool {
    let guard = GRID.read().unwrap();
    let Some(g) = guard.as_ref() else {
        return false;
    };
    g.low
        .get(&(x.div_euclid(CELL), z.div_euclid(CELL)))
        .copied()
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lookups must map a world point to its world lattice cell, independent of which
    /// area built the grid.
    #[test]
    fn lookup_is_world_anchored() {
        let mut low = HashMap::new();
        low.insert((3, -2), true);
        *GRID.write().unwrap() = Some(LowlandGrid { low });
        // 3*128 through 3*128+127 all resolve to lattice cell 3; -2 covers -256..-129.
        assert!(is_lowland(3 * CELL, -2 * CELL));
        assert!(is_lowland(3 * CELL + 127, -CELL - 1));
        assert!(!is_lowland(4 * CELL, -2 * CELL));
        *GRID.write().unwrap() = None;
    }
}
