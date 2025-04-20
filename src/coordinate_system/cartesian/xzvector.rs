use serde::Deserialize;
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct XZVector {
    pub dx: i32,
    pub dz: i32,
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
