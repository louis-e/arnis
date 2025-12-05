use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// An underlying shape of `XZBBox` enum.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct XZBBoxRect {
    /// The "bottom-left" vertex of the rectangle
    min: XZPoint,

    /// The "top-right" vertex of the rectangle
    max: XZPoint,
}

impl XZBBoxRect {
    pub fn new(min: XZPoint, max: XZPoint) -> Result<Self, String> {
        let blockx_ge_1 = max.x - min.x >= 0;
        let blockz_ge_1 = max.z - min.z >= 0;

        if !blockx_ge_1 {
            return Err(format!(
                "Invalid XZBBox::Rect: max.x should >= min.x, but encountered {} -> {}",
                min.x, max.x
            ));
        }

        if !blockz_ge_1 {
            return Err(format!(
                "Invalid XZBBox::Rect: max.z should >= min.z, but encountered {} -> {}",
                min.z, max.z
            ));
        }

        Ok(Self { min, max })
    }

    pub fn min(&self) -> XZPoint {
        self.min
    }

    pub fn max(&self) -> XZPoint {
        self.max
    }

    /// Total number of blocks covered in this 2D bbox
    pub fn total_blocks(&self) -> u64 {
        u64::from(self.total_blocks_x()) * u64::from(self.total_blocks_z())
    }

    /// Total number of blocks covered in x direction
    pub fn total_blocks_x(&self) -> u32 {
        let nx = self.max.x - self.min.x + 1;
        nx as u32
    }

    /// Total number of blocks covered in z direction
    pub fn total_blocks_z(&self) -> u32 {
        let nz = self.max.z - self.min.z + 1;
        nz as u32
    }

    /// Check whether an `XZPoint` is covered
    pub fn contains(&self, xzpoint: XZPoint) -> bool {
        xzpoint.x >= self.min.x
            && xzpoint.x <= self.max.x
            && xzpoint.z >= self.min.z
            && xzpoint.z <= self.max.z
    }
}

impl fmt::Display for XZBBoxRect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rect({} -> {})", self.min, self.max)
    }
}

// below are associated +- operators
impl Add<XZVector> for XZBBoxRect {
    type Output = XZBBoxRect;

    fn add(self, other: XZVector) -> Self {
        Self {
            min: self.min + other,
            max: self.max + other,
        }
    }
}

impl AddAssign<XZVector> for XZBBoxRect {
    fn add_assign(&mut self, other: XZVector) {
        self.min += other;
        self.max += other;
    }
}

impl Sub<XZVector> for XZBBoxRect {
    type Output = XZBBoxRect;

    fn sub(self, other: XZVector) -> Self {
        Self {
            min: self.min - other,
            max: self.max - other,
        }
    }
}

impl SubAssign<XZVector> for XZBBoxRect {
    fn sub_assign(&mut self, other: XZVector) {
        self.min -= other;
        self.max -= other;
    }
}
