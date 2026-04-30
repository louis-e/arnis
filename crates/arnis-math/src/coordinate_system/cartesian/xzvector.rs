use serde::Deserialize;
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// Vector between two points in minecraft xz space.
#[derive(Debug, Deserialize, Copy, Clone, PartialEq)]
pub struct XZVector {
    /// Increment in x direction
    pub dx: i32,

    /// Increment in z direction
    pub dz: i32,
}

impl fmt::Display for XZVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XZVector({}, {})", self.dx, self.dz)
    }
}

// below are associated +- operators
impl Add for XZVector {
    type Output = XZVector;

    fn add(self, other: XZVector) -> XZVector {
        XZVector {
            dx: self.dx + other.dx,
            dz: self.dz + other.dz,
        }
    }
}

impl AddAssign for XZVector {
    fn add_assign(&mut self, other: XZVector) {
        self.dx += other.dx;
        self.dz += other.dz;
    }
}

impl Sub for XZVector {
    type Output = XZVector;

    fn sub(self, other: XZVector) -> XZVector {
        XZVector {
            dx: self.dx - other.dx,
            dz: self.dz - other.dz,
        }
    }
}

impl SubAssign for XZVector {
    fn sub_assign(&mut self, other: XZVector) {
        self.dx -= other.dx;
        self.dz -= other.dz;
    }
}
