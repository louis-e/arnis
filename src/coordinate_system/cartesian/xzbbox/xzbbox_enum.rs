use super::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone)]
pub enum XZBBox {
    Rect(XZBBoxRect),
}

impl XZBBox {
    pub fn from_scale_factors(scale_factor_x: f64, scale_factor_z: f64) -> Self {
        Self::Rect(XZBBoxRect {
            point1: XZPoint { x: 0, z: 0 },
            point2: XZPoint {
                x: scale_factor_x as i32,
                z: scale_factor_z as i32,
            },
        })
    }

    pub fn contains(&self, xzpoint: XZPoint) -> bool {
        match self {
            Self::Rect(r) => r.contains(xzpoint),
        }
    }

    pub fn circumscribed_rect(&self) -> XZBBoxRect {
        match self {
            Self::Rect(r) => *r,
        }
    }

    pub fn min_x(&self) -> i32 {
        self.circumscribed_rect().point1.x
    }

    pub fn max_x(&self) -> i32 {
        self.circumscribed_rect().point2.x
    }

    pub fn min_z(&self) -> i32 {
        self.circumscribed_rect().point1.z
    }

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
