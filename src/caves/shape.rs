//! The carve as a pure function of position, readable past the edge of the region being carved.
//!
//! Pools, rivers and geodes reach well past their origin chunk, so one near a tile edge is seen by
//! both tiles. They only come out whole if both tiles make the same decisions about them, which
//! they can do only against something both see identically. The tile's own cave-air set stops at
//! its edge and has been despeckled and pruned, so it cannot be that. [`CaveShape`] answers "does
//! the carve open this block?" from the density field and the carvers alone: exactly the carve
//! before despeckle and pruning, anywhere around the region.

use super::carver::{self, Ellipsoid, CAVE_RANGE_CHUNKS};
use super::density::CaveGen;
use super::{lerp, CARVE_THRESHOLD, CELL_H, CELL_W, TOP_GATE};
use crate::world_editor::terrain_floor_y;
use fnv::FnvHashMap;
use std::cell::RefCell;

/// How far a feature reaches from its origin: a river walks at most 52 one-block steps and carves
/// up to 3 blocks around its path; pools (19) and geodes (7) stay well inside this.
pub(super) const FEATURE_REACH: i32 = 64;

/// An inclusive X/Z block rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rect {
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }

    pub fn grow(&self, d: i32) -> Rect {
        Rect {
            min_x: self.min_x - d,
            max_x: self.max_x + d,
            min_z: self.min_z - d,
            max_z: self.max_z + d,
        }
    }

    pub fn clip(&self, other: Rect) -> Rect {
        Rect {
            min_x: self.min_x.max(other.min_x),
            max_x: self.max_x.min(other.max_x),
            min_z: self.min_z.max(other.min_z),
            max_z: self.max_z.min(other.max_z),
        }
    }

    /// The chunks the rect touches, as `(cx0, cx1, cz0, cz1)`.
    pub fn chunks(&self) -> (i32, i32, i32, i32) {
        (
            self.min_x.div_euclid(16),
            self.max_x.div_euclid(16),
            self.min_z.div_euclid(16),
            self.max_z.div_euclid(16),
        )
    }
}

pub(super) struct CaveShape<'g> {
    gen: &'g CaveGen,
    /// Features never leave the world's bbox, whichever tile plans them.
    world: Rect,
    /// Every block a feature that can reach the region may look at.
    ext: Rect,
    /// Surface height over `ext`, X-major.
    surf: Vec<i32>,
    floor: i32,
    y_shift: i32,
    /// Carver ellipsoids by the chunk columns they overlap.
    carvers: FnvHashMap<(i32, i32), Vec<Ellipsoid>>,
    /// Density at cell corners, computed on first use. The shape lives inside one tile's pass,
    /// so a plain RefCell is enough.
    corners: RefCell<FnvHashMap<(i32, i32, i32), f64>>,
}

impl<'g> CaveShape<'g> {
    pub fn new(
        gen: &'g CaveGen,
        seed: i64,
        world: Rect,
        region: Rect,
        surf_at: impl Fn(i32, i32) -> i32,
    ) -> Self {
        // A feature whose origin chunk lies within FEATURE_REACH of the region can touch it, and
        // it looks up to FEATURE_REACH around that origin.
        let ext = region.grow(2 * FEATURE_REACH + 16).clip(world);
        let h = (ext.max_z - ext.min_z + 1) as usize;
        let mut surf = Vec::with_capacity((ext.max_x - ext.min_x + 1) as usize * h);
        for x in ext.min_x..=ext.max_x {
            for z in ext.min_z..=ext.max_z {
                surf.push(surf_at(x, z));
            }
        }

        let (cx0, cx1, cz0, cz1) = ext.chunks();
        let mut carvers: FnvHashMap<(i32, i32), Vec<Ellipsoid>> = FnvHashMap::default();
        for e in carver::ellipsoids(
            seed,
            cx0 - CAVE_RANGE_CHUNKS,
            cx1 + CAVE_RANGE_CHUNKS,
            cz0 - CAVE_RANGE_CHUNKS,
            cz1 + CAVE_RANGE_CHUNKS,
        ) {
            let (x0, x1) = e.x_span();
            let (z0, z1) = e.z_span();
            for bx in x0.max(ext.min_x).div_euclid(16)..=x1.min(ext.max_x).div_euclid(16) {
                for bz in z0.max(ext.min_z).div_euclid(16)..=z1.min(ext.max_z).div_euclid(16) {
                    carvers.entry((bx, bz)).or_default().push(e);
                }
            }
        }

        CaveShape {
            gen,
            world,
            ext,
            surf,
            floor: terrain_floor_y(),
            y_shift: super::y_shift(),
            carvers,
            corners: RefCell::new(FnvHashMap::default()),
        }
    }

    pub fn world(&self) -> Rect {
        self.world
    }

    /// The bedrock plane.
    pub fn floor(&self) -> i32 {
        self.floor
    }

    /// Surface height of a column near the region.
    pub fn surf(&self, x: i32, z: i32) -> i32 {
        debug_assert!(
            self.ext.contains(x, z),
            "({x}, {z}) is outside {:?}",
            self.ext
        );
        if !self.ext.contains(x, z) {
            return self.floor;
        }
        let h = (self.ext.max_z - self.ext.min_z + 1) as usize;
        self.surf[(x - self.ext.min_x) as usize * h + (z - self.ext.min_z) as usize]
    }

    /// Highest block the carve may open in a column (the roof seal).
    pub fn top(&self, x: i32, z: i32) -> i32 {
        self.surf(x, z) - TOP_GATE
    }

    /// Whether the carve (noise caves and carvers, before despeckle and pruning) opens a block.
    pub fn is_cave(&self, x: i32, y: i32, z: i32) -> bool {
        if !self.ext.contains(x, z) || y <= self.floor || y > self.top(x, z) {
            return false;
        }
        let carved = self
            .carvers
            .get(&(x.div_euclid(16), z.div_euclid(16)))
            .is_some_and(|ells| ells.iter().any(|e| e.contains(x, y - self.y_shift, z)));
        carved || self.noise_carves(x, y, z)
    }

    /// The noise carve at one block, interpolated exactly as `mod.rs` does over its cells.
    fn noise_carves(&self, x: i32, y: i32, z: i32) -> bool {
        let (wx0, wy0, wz0) = (
            x.div_euclid(CELL_W) * CELL_W,
            y.div_euclid(CELL_H) * CELL_H,
            z.div_euclid(CELL_W) * CELL_W,
        );
        let (wx1, wy1, wz1) = (wx0 + CELL_W, wy0 + CELL_H, wz0 + CELL_W);
        let mut corners = self.corners.borrow_mut();
        let mut corner = |cx: i32, cy: i32, cz: i32| {
            *corners
                .entry((cx, cy, cz))
                .or_insert_with(|| self.gen.combined_density(cx, cy, cz))
        };
        let (n000, n100, n001, n101) = (
            corner(wx0, wy0, wz0),
            corner(wx1, wy0, wz0),
            corner(wx0, wy0, wz1),
            corner(wx1, wy0, wz1),
        );
        let (n010, n110, n011, n111) = (
            corner(wx0, wy1, wz0),
            corner(wx1, wy1, wz0),
            corner(wx0, wy1, wz1),
            corner(wx1, wy1, wz1),
        );
        drop(corners);
        let fy = (y - wy0) as f64 / CELL_H as f64;
        let xz00 = lerp(fy, n000, n010);
        let xz10 = lerp(fy, n100, n110);
        let xz01 = lerp(fy, n001, n011);
        let xz11 = lerp(fy, n101, n111);
        let fx = (x - wx0) as f64 / CELL_W as f64;
        let z0v = lerp(fx, xz00, xz10);
        let z1v = lerp(fx, xz01, xz11);
        let fz = (z - wz0) as f64 / CELL_W as f64;
        let combined = lerp(fz, z0v, z1v);
        combined <= CARVE_THRESHOLD || self.gen.noodle_density(x, y, z) <= 0.0
    }
}
