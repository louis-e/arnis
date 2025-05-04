use super::xzvector::XZVector;
use serde::Deserialize;
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Debug, Deserialize, Copy, Clone, PartialEq, Hash)]
pub struct XZPoint {
    pub x: i32,
    pub z: i32,
}

impl XZPoint {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    // pub fn origin() -> Self {
    //     Self {x: 0, z: 0}
    // }

    pub fn to_relative(&self, relorigin: XZPoint) -> XZPoint {
        Self {
            x: self.x - relorigin.x,
            z: self.z - relorigin.z,
        }
    }

    // pub fn to_absolute(&self, relorigin: XZPoint) -> XZPoint {
    //     Self {x: self.x + relorigin.x, z: self.z + relorigin.z}
    // }
}

impl fmt::Display for XZPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XZPoint({}, {})", self.x, self.z)
    }
}

// below are associated +- operators
impl Add<XZVector> for XZPoint {
    type Output = XZPoint;

    fn add(self, other: XZVector) -> XZPoint {
        XZPoint {
            x: self.x + other.dx,
            z: self.z + other.dz,
        }
    }
}

impl AddAssign<XZVector> for XZPoint {
    fn add_assign(&mut self, other: XZVector) {
        self.x += other.dx;
        self.z += other.dz;
    }
}

impl Sub for XZPoint {
    type Output = XZVector;

    fn sub(self, other: XZPoint) -> XZVector {
        XZVector {
            dx: self.x - other.x,
            dz: self.z - other.z,
        }
    }
}

impl Sub<XZVector> for XZPoint {
    type Output = XZPoint;

    fn sub(self, other: XZVector) -> XZPoint {
        XZPoint {
            x: self.x - other.dx,
            z: self.z - other.dz,
        }
    }
}

impl SubAssign<XZVector> for XZPoint {
    fn sub_assign(&mut self, other: XZVector) {
        self.x -= other.dx;
        self.z -= other.dz;
    }
}
