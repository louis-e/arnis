use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Copy, Clone)]
pub struct XZBBoxRect {
    pub point1: XZPoint,
    pub point2: XZPoint,
}

impl XZBBoxRect {
    pub fn total_blocks(&self) -> u64 {
        (self.total_blocks_x() as u64) * (self.total_blocks_z() as u64)
    }

    pub fn total_blocks_x(&self) -> u32 {
        let nx = self.point2.x - self.point1.x + 1;
        nx as u32
    }

    pub fn total_blocks_z(&self) -> u32 {
        let nz = self.point2.z - self.point1.z + 1;
        nz as u32
    }

    pub fn contains(&self, xzpoint: XZPoint) -> bool {
        xzpoint.x >= self.point1.x
            && xzpoint.x <= self.point2.x
            && xzpoint.z >= self.point1.z
            && xzpoint.z <= self.point2.z
    }
}

// below are associated +- operators
impl Add<XZVector> for XZBBoxRect {
    type Output = XZBBoxRect;

    fn add(self, other: XZVector) -> Self {
        Self {
            point1: self.point1 + other,
            point2: self.point2 + other,
        }
    }
}

impl AddAssign<XZVector> for XZBBoxRect {
    fn add_assign(&mut self, other: XZVector) {
        self.point1 += other;
        self.point2 += other;
    }
}

impl Sub<XZVector> for XZBBoxRect {
    type Output = XZBBoxRect;

    fn sub(self, other: XZVector) -> Self {
        Self {
            point1: self.point1 - other,
            point2: self.point2 - other,
        }
    }
}

impl SubAssign<XZVector> for XZBBoxRect {
    fn sub_assign(&mut self, other: XZVector) {
        self.point1 -= other;
        self.point2 -= other;
    }
}
