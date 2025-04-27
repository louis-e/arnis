use super::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// Bounding Box in minecraft XZ space with varied shapes.
#[derive(Clone, Debug)]
pub enum XZBBox {
    Rect(XZBBoxRect),
}

impl XZBBox {
    /// Construct rectangle shape bbox from the x and z lengths of the world, originated at (0, 0)
    pub fn rect_from_xz_lengths(length_x: f64, length_z: f64) -> Result<Self, String> {
        let len_ge_1 = length_x >= 1.0 && length_z >= 1.0;

        if !len_ge_1 {
            return Err("Invalid XZBBox: World length in x and z should both >= 1.0".to_string());
        }

        Ok(Self::Rect(XZBBoxRect {
            point1: XZPoint { x: 0, z: 0 },
            point2: XZPoint {
                x: length_x as i32,
                z: length_z as i32,
            },
        }))
    }

    /// Check whether an XZPoint is covered
    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
        match self {
            Self::Rect(r) => r.contains(xzpoint),
        }
    }

    /// Return the circumscribed rectangle of the current XZBBox shape
    pub fn circumscribed_rect(&self) -> XZBBoxRect {
        match self {
            Self::Rect(r) => *r,
        }
    }

    /// Return the min x in all covered blocks
    pub fn min_x(&self) -> i32 {
        self.circumscribed_rect().point1.x
    }

    /// Return the max x in all covered blocks
    pub fn max_x(&self) -> i32 {
        self.circumscribed_rect().point2.x
    }

    /// Return the min z in all covered blocks
    pub fn min_z(&self) -> i32 {
        self.circumscribed_rect().point1.z
    }

    /// Return the max z in all covered blocks
    pub fn max_z(&self) -> i32 {
        self.circumscribed_rect().point2.z
    }
}

// below are associated +- operators
impl Add<XZVector> for XZBBox {
    type Output = XZBBox;

    fn add(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r + other),
        }
    }
}

impl AddAssign<XZVector> for XZBBox {
    fn add_assign(&mut self, other: XZVector) {
        match self {
            Self::Rect(r) => *r += other,
        }
    }
}

impl Sub<XZVector> for XZBBox {
    type Output = XZBBox;

    fn sub(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r - other),
        }
    }
}

impl SubAssign<XZVector> for XZBBox {
    fn sub_assign(&mut self, other: XZVector) {
        match self {
            Self::Rect(r) => *r -= other,
        }
    }
}
