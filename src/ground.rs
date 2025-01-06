use crate::cartesian::XZPoint;

pub struct Ground {
    elevation_enabled: bool,
    ground_level: i32,
}

impl Ground {
    /// Creates a new `Ground` instance with an elevation toggle
    pub fn new(elevation_enabled: bool, ground_level: i32) -> Self {
        Self {
            elevation_enabled,
            ground_level,
        }
    }

    /// Returns the ground level at a given point
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if self.elevation_enabled {
            // Use sinusoidal terrain if elevation is enabled
            (20.0 * (coord.x as f64 / 20.0).sin() + 20.0 * (coord.z as f64 / 20.0).sin() - 40.0)
                as i32
        } else {
            // Flat terrain
            self.ground_level
        }
    }

    /// Returns the minimum ground level from a list of points
    #[inline(always)]
    pub fn min_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }

        coords.map(|c: XZPoint| self.level(c)).min()
    }

    /// Returns the maximum ground level from a list of points
    #[inline(always)]
    pub fn max_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }

        coords.map(|c: XZPoint| self.level(c)).max()
    }
}