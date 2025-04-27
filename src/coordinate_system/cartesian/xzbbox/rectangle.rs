use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// An underlying shape of XZBBox enum.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct XZBBoxRect {
    /// The "bottom-left" vertex of the rectangle
    point1: XZPoint,

    /// The "top-right" vertex of the rectangle
    point2: XZPoint,
}

impl XZBBoxRect {
    pub fn new(point1: XZPoint, point2: XZPoint) -> Result<Self, String> {
        let obj = Self { point1, point2 };

        let blockx_ge_1 = obj.total_blocks_x() >= 1;
        let blockz_ge_1 = obj.total_blocks_z() >= 1;

        if !blockx_ge_1 || !blockz_ge_1 {
            return Err(
                "Invalid XZBBox: Total number of blocks in x and z should both > 1".to_string(),
            );
        }

        Ok(obj)
    }

    pub fn point1(&self) -> XZPoint {
        self.point1
    }

    pub fn point2(&self) -> XZPoint {
        self.point2
    }

    /// Total number of blocks covered in this 2D bbox
    pub fn total_blocks(&self) -> u64 {
        (self.total_blocks_x() as u64) * (self.total_blocks_z() as u64)
    }

    /// Total number of blocks covered in x direction
    pub fn total_blocks_x(&self) -> u32 {
        let nx = self.point2.x - self.point1.x + 1;
        nx as u32
    }

    /// Total number of blocks covered in z direction
    pub fn total_blocks_z(&self) -> u32 {
        let nz = self.point2.z - self.point1.z + 1;
        nz as u32
    }

    /// Check whether an XZPoint is covered
    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
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
